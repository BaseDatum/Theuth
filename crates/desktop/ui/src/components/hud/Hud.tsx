import { Link } from '@tanstack/react-router'
import { Activity as ActivityIcon, Bell, Bot, ListChecks, Network, PanelLeft, Wallet, Workflow } from 'lucide-react'
import { type ReactNode, useEffect, useState } from 'react'
import { Composer } from '@/components/chat/Composer'
import { useRoutines } from '@/components/routines/queries'
import { SearchTrigger } from '@/components/sidebar/SearchTrigger'
import { age } from '@/components/tasks/helpers'
import { useAgents, useMesh, useTasks } from '@/components/tasks/queries'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { useSidebar } from '@/components/ui/sidebar'
import { Skeleton } from '@/components/ui/skeleton'
import { isMacDesktop } from '@/lib/hotkeys'
import { cn } from '@/lib/utils'
import type { ActivityEntry } from '@/rpc/bindings/ActivityEntry'
import type { AgentSummary } from '@/rpc/bindings/AgentSummary'
import type { AttentionItem } from '@/rpc/bindings/AttentionItem'
import type { ScheduleInfo } from '@/rpc/bindings/ScheduleInfo'
import type { Task } from '@/rpc/bindings/Task'
import { boardPulse, headline, inFlight, runningRoutines, snoozeUntil, spend } from './model'
import { useActivity, useAttention, useAttentionActions } from './queries'

/**
 * `/` — the THEUTH mark's overview (70 HUD, paperclip's dashboard + attention inbox), Sky's order top → bottom:
 * headline numbers · Attention (decisions inline) · In flight · Board pulse · Activity (live) · Spend + mesh · the
 * new-chat composer docked at the bottom. Every section has its own query, so they load in parallel behind skeletons.
 */
export function Hud() {
  const sidebar = useSidebar()
  const mac = isMacDesktop()
  const attention = useAttention()
  const tasks = useTasks()
  const agents = useAgents()
  const routines = useRoutines()
  const activity = useActivity()
  const mesh = useMesh()
  useEffect(() => {
    document.title = ''
  }, [])
  const h = headline(agents.data ?? [], tasks.data ?? [], attention.data ?? [])
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
        <h2 className="min-w-0 flex-1 truncate text-sm font-semibold">Overview</h2>
      </header>
      <div className="min-h-0 flex-1 overflow-y-auto">
        <div className="mx-auto flex w-full max-w-[880px] flex-col gap-5 p-4">
          <div className="grid grid-cols-2 gap-2 sm:grid-cols-4">
            <Metric label="Agents active" value={agents.data ? String(h.agentsActive) : null} to="/agents" />
            <Metric label="Tasks in progress" value={tasks.data ? String(h.tasksInProgress) : null} to="/tasks" search={{ state: 'doing', project: '' }} />
            <Metric label="Month spend" value={agents.data ? `$${h.monthSpend.toFixed(2)}` : null} />
            <Metric label="Pending attention" value={attention.data ? String(h.pending) : null} tone={h.pending > 0 ? 'warning' : undefined} />
          </div>

          <Section icon={<Bell className="size-3.5" />} title="Attention" count={attention.data?.length} pending={attention.isPending} error={attention.error}>
            <AttentionList items={attention.data} />
          </Section>

          <Section icon={<Workflow className="size-3.5" />} title="In flight" pending={tasks.isPending || routines.isPending} error={tasks.error ?? routines.error}>
            <InFlightList tasks={tasks.data} routines={routines.data} />
          </Section>

          <Section icon={<ListChecks className="size-3.5" />} title="Board pulse" pending={tasks.isPending} error={tasks.error}>
            {tasks.data && (
              <div className="flex flex-wrap gap-2">
                {boardPulse(tasks.data).map(({ state, n }) => (
                  <Link
                    key={state}
                    to="/tasks"
                    search={{ state, project: '' }}
                    className={cn('flex items-center gap-2 rounded-full border border-border-subtle px-3 py-1 text-[13px] hover:bg-accent', n === 0 && 'text-fg-tertiary')}
                  >
                    <span className="tabular-nums font-semibold">{n}</span> {state}
                  </Link>
                ))}
                {tasks.data.length === 0 && <EmptyNote>No tasks yet. Counts per column appear as tasks are created; each chip opens the board filtered to that state.</EmptyNote>}
              </div>
            )}
          </Section>

          <Section icon={<ActivityIcon className="size-3.5" />} title="Activity" count={activity.data?.length} pending={activity.isPending} error={activity.error}>
            {activity.data?.length === 0 && <EmptyNote>Quiet so far. Task moves, comments, check-outs, hires, routine fires and escalations stream in here as they happen (last 50).</EmptyNote>}
            {activity.data && activity.data.length > 0 && (
              <ul className="divide-y divide-border-subtle rounded-lg border border-border-subtle">
                {activity.data.map((e) => (
                  <ActivityRow key={e.id} entry={e} />
                ))}
              </ul>
            )}
          </Section>

          <Section
            icon={<Wallet className="size-3.5" />}
            title="Spend"
            pending={agents.isPending}
            error={agents.error}
            actions={
              mesh.data && (
                <Link to="/settings/$tab" params={{ tab: 'mesh' }} className="flex items-center gap-1.5 rounded-full border border-border-subtle px-2.5 py-0.5 text-xs text-fg-secondary hover:bg-accent">
                  <Network className="size-3" /> {mesh.data.peers.filter((p) => p.reachable).length + 1}/{mesh.data.peers.length + 1} nodes
                  {mesh.data.lease.holder !== null && <span className="text-fg-tertiary">· hub {mesh.data.lease.holder === mesh.data.self_id ? 'here' : (mesh.data.peers.find((p) => p.id === mesh.data.lease.holder)?.name ?? 'elsewhere')}</span>}
                </Link>
              )
            }
          >
            <SpendList agents={agents.data} />
          </Section>
        </div>
      </div>
      <div className="shrink-0 border-t border-border-subtle px-4 pt-3">
        <Composer session={null} draftCreate={{ private: false, project: null }} compact />
      </div>
    </article>
  )
}

function Metric({ label, value, tone, to, search }: { label: string; value: string | null; tone?: 'warning'; to?: '/agents' | '/tasks'; search?: { state: 'doing'; project: '' } }) {
  const body = (
    <>
      <span className="text-xs text-fg-tertiary">{label}</span>
      {value === null ? <Skeleton className="mt-1 h-7 w-16" /> : <span className={cn('text-2xl font-semibold tabular-nums', tone === 'warning' && 'text-warning')}>{value}</span>}
    </>
  )
  const cls = 'flex flex-col rounded-lg border border-border-subtle bg-layer-1 px-3 py-2'
  if (to === '/tasks') return <Link to={to} search={search} className={cn(cls, 'hover:bg-accent')}>{body}</Link>
  if (to === '/agents') return <Link to={to} className={cn(cls, 'hover:bg-accent')}>{body}</Link>
  return <div className={cls}>{body}</div>
}

export function Section({ icon, title, count, pending, error, actions, children }: { icon: ReactNode; title: string; count?: number; pending: boolean; error: Error | null; actions?: ReactNode; children: ReactNode }) {
  return (
    <section aria-label={title}>
      <h3 className="mb-2 flex items-center gap-2 text-xs font-semibold uppercase tracking-wide text-fg-secondary">
        {icon} {title}
        {count !== undefined && count > 0 && <span className="font-normal tabular-nums text-fg-tertiary">{count}</span>}
        <span className="ml-auto">{actions}</span>
      </h3>
      {pending && (
        <div className="space-y-2">
          <Skeleton className="h-10 w-full" />
          <Skeleton className="h-10 w-full" />
        </div>
      )}
      {error && <p className="text-sm text-fg-tertiary">{error.message}</p>}
      {!pending && !error && children}
    </section>
  )
}

function EmptyNote({ children }: { children: ReactNode }) {
  return <p className="rounded-lg border border-dashed border-border-subtle px-3 py-3 text-sm text-fg-tertiary">{children}</p>
}

const KIND_LABEL: Record<AttentionItem['kind'], string> = {
  approval: 'approval',
  decision: 'decision',
  hire: 'hire',
  blocked: 'blocked',
  review: 'review',
  budget: 'budget',
  node: 'node',
}

/** The inbox rows, or what will land in them. Shared by the HUD section and `/attention`. */
export function AttentionList({ items }: { items: AttentionItem[] | undefined }) {
  if (!items || items.length === 0) {
    return <EmptyNote>Nothing needs you. Tool approvals, decisions agents escalate, hires, blocked or review tasks, budgets at 80 % and unreachable nodes land here.</EmptyNote>
  }
  return (
    <ul className="divide-y divide-border-subtle rounded-lg border border-border-subtle">
      {items.map((i) => (
        <AttentionCard key={i.id} item={i} />
      ))}
    </ul>
  )
}

/** One inbox row with its decision inline: allow/deny · answer · hire/decline · done/back · start/unblock · un-pause; snooze and dismiss on every row. */
function AttentionCard({ item }: { item: AttentionItem }) {
  const act = useAttentionActions()
  const [answer, setAnswer] = useState('')
  const busy = Object.values(act).some((a) => a.isPending)
  return (
    <li className="flex flex-col gap-2 px-3 py-2 text-[13px] sm:flex-row sm:items-center sm:gap-3">
      <div className="min-w-0 flex-1">
        <div className="flex items-center gap-2">
          <span className="rounded-full bg-layer-2 px-2 py-0.5 text-[11px] uppercase tracking-wide text-fg-secondary">{KIND_LABEL[item.kind]}</span>
          {item.task ? (
            <Link to="/tasks/$id" params={{ id: item.task }} className="truncate font-medium hover:underline">
              {item.title}
            </Link>
          ) : item.agent ? (
            <Link to="/agents/$name/$tab" params={{ name: item.agent, tab: 'overview' }} className="truncate font-medium hover:underline">
              {item.title}
            </Link>
          ) : item.session ? (
            <Link to="/c/$session" params={{ session: item.session }} className="truncate font-medium hover:underline">
              {item.title}
            </Link>
          ) : (
            <span className="truncate font-medium">{item.title}</span>
          )}
          <span className="ml-auto shrink-0 text-xs text-fg-tertiary tabular-nums">{age(item.at_ms)}</span>
        </div>
        {item.detail && <p className="truncate text-fg-secondary">{item.detail}</p>}
      </div>
      <div className="flex shrink-0 flex-wrap items-center gap-1.5">
        {item.kind === 'approval' && item.approval && (
          <>
            <Button size="xs" disabled={busy} onClick={() => act.approve.mutate({ id: item.approval as NonNullable<typeof item.approval>, allow: true })}>
              Allow
            </Button>
            <Button size="xs" variant="outline" disabled={busy} onClick={() => act.approve.mutate({ id: item.approval as NonNullable<typeof item.approval>, allow: false })}>
              Deny
            </Button>
          </>
        )}
        {item.kind === 'decision' && item.decision && (
          <form
            className="flex items-center gap-1.5"
            onSubmit={(e) => {
              e.preventDefault()
              if (answer.trim() && item.decision) act.decide.mutate({ decision: item.decision, answer: answer.trim() })
            }}
          >
            <Input value={answer} onChange={(e) => setAnswer(e.target.value)} placeholder="Your decision…" className="h-7 w-44 text-base md:text-xs" aria-label="Decision" />
            <Button size="xs" type="submit" disabled={busy || !answer.trim()}>
              Answer
            </Button>
          </form>
        )}
        {item.kind === 'hire' && item.agent && (
          <>
            <Button size="xs" disabled={busy} onClick={() => act.hire.mutate(item.agent as string)}>
              Hire
            </Button>
            <Button size="xs" variant="outline" disabled={busy} onClick={() => act.decline.mutate(item.agent as string)}>
              Decline
            </Button>
          </>
        )}
        {item.kind === 'review' && item.task && (
          <>
            <Button size="xs" disabled={busy} onClick={() => act.move.mutate({ id: item.task as string, state: 'done' })}>
              Move done
            </Button>
            <Button size="xs" variant="outline" disabled={busy} onClick={() => act.move.mutate({ id: item.task as string, state: 'doing' })}>
              Send back
            </Button>
          </>
        )}
        {item.kind === 'blocked' && item.task && (
          <Button size="xs" variant="outline" disabled={busy} onClick={() => act.move.mutate({ id: item.task as string, state: 'todo' })}>
            Unblock
          </Button>
        )}
        {item.kind === 'budget' && item.agent && (
          <Button size="xs" variant="outline" disabled={busy} onClick={() => act.unpause.mutate(item.agent as string)}>
            Un-pause
          </Button>
        )}
        <Button size="xs" variant="ghost" disabled={busy} title="Snooze until 09:00 tomorrow" onClick={() => act.snooze.mutate({ id: item.id, until: snoozeUntil('tomorrow') })}>
          Snooze
        </Button>
        <Button size="xs" variant="ghost" disabled={busy} onClick={() => act.dismiss.mutate(item.id)}>
          Dismiss
        </Button>
      </div>
    </li>
  )
}

function InFlightList({ tasks, routines }: { tasks: Task[] | undefined; routines: ScheduleInfo[] | undefined }) {
  const rows = inFlight(tasks ?? [])
  const running = runningRoutines(routines ?? [])
  if (rows.length === 0 && running.length === 0) {
    return <EmptyNote>Nothing is running. Desks at work show here as agent · task · session · elapsed, with any routine mid-run.</EmptyNote>
  }
  return (
    <ul className="divide-y divide-border-subtle rounded-lg border border-border-subtle">
      {rows.map((r) => (
        <li key={r.task.id} className="flex items-center gap-3 px-3 py-2 text-[13px]">
          <Bot className="size-3.5 shrink-0 text-fg-tertiary" />
          <span className="shrink-0 font-medium">{r.agent}</span>
          <Link to="/tasks/$id" params={{ id: r.task.id }} className="min-w-0 flex-1 truncate hover:underline">
            {r.task.title}
          </Link>
          {r.session && (
            <Link to="/c/$session" params={{ session: r.session }} className="shrink-0 font-mono text-xs text-fg-secondary hover:underline">
              {r.session.slice(-6)}
              {r.engine ? ` @ ${r.engine}` : ''}
            </Link>
          )}
          <span className="shrink-0 text-xs text-fg-tertiary tabular-nums">{age(r.sinceMs)}</span>
        </li>
      ))}
      {running.map((s) => (
        <li key={`routine:${s.name}`} className="flex items-center gap-3 px-3 py-2 text-[13px]">
          <Workflow className="size-3.5 shrink-0 text-fg-tertiary" />
          <span className="shrink-0 font-medium">routine</span>
          <Link to="/routines/$name" params={{ name: s.name }} className="min-w-0 flex-1 truncate hover:underline">
            {s.name}
          </Link>
          {s.session && (
            <Link to="/c/$session" params={{ session: s.session }} className="shrink-0 font-mono text-xs text-fg-secondary hover:underline">
              {s.session.slice(-6)}
            </Link>
          )}
          <span className="shrink-0 text-xs text-fg-tertiary">running</span>
        </li>
      ))}
    </ul>
  )
}

function ActivityRow({ entry }: { entry: ActivityEntry }) {
  const kind = entry.kind.replace(/_/g, ' ')
  const isTask = entry.kind.startsWith('task_') || entry.kind.startsWith('child_') || entry.kind.startsWith('escalation_')
  return (
    <li className="flex items-center gap-3 px-3 py-1.5 text-[13px]">
      <span className="w-28 shrink-0 truncate text-xs text-fg-tertiary">{kind}</span>
      <span className="shrink-0 font-medium">{entry.actor.replace(/^agent:/, '')}</span>
      {isTask ? (
        <Link to="/tasks/$id" params={{ id: entry.subject }} className="min-w-0 flex-1 truncate hover:underline">
          {entry.summary}
        </Link>
      ) : (
        <span className="min-w-0 flex-1 truncate">{entry.summary}</span>
      )}
      {entry.cost !== null && <span className="shrink-0 text-xs text-fg-tertiary tabular-nums">${entry.cost.toFixed(2)}</span>}
      <span className="shrink-0 text-xs text-fg-tertiary tabular-nums">{age(entry.ts_ms)}</span>
    </li>
  )
}

function SpendList({ agents }: { agents: AgentSummary[] | undefined }) {
  const rows = spend(agents ?? [])
  if (rows.length === 0) return <EmptyNote>No spend this month. Each agent's month total appears here against its budget line once its sessions cost something.</EmptyNote>
  return (
    <ul className="divide-y divide-border-subtle rounded-lg border border-border-subtle">
      {rows.map((r) => (
        <li key={r.name} className="flex items-center gap-3 px-3 py-2 text-[13px]">
          <Link to="/agents/$name/$tab" params={{ name: r.name, tab: 'overview' }} className="w-28 shrink-0 truncate font-medium hover:underline">
            {r.name}
          </Link>
          <div className="min-w-0 flex-1">
            {r.share !== null ? (
              <div className="h-1.5 w-full overflow-hidden rounded-full bg-layer-2" role="progressbar" aria-valuenow={Math.round(r.share * 100)} aria-valuemin={0} aria-valuemax={100}>
                <div className={cn('h-full rounded-full', r.share >= 1 ? 'bg-danger' : r.share >= 0.8 ? 'bg-warning' : 'bg-primary')} style={{ width: `${Math.round(r.share * 100)}%` }} />
              </div>
            ) : (
              <span className="text-xs text-fg-tertiary">no budget</span>
            )}
          </div>
          <span className="shrink-0 text-xs tabular-nums text-fg-secondary">
            {r.spent.toFixed(2)}
            {r.cap !== null ? ` / ${r.cap.toFixed(2)}` : ''} {r.currency}
            {r.paused ? ' · paused' : ''}
          </span>
        </li>
      ))}
    </ul>
  )
}
