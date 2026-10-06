import { useQueryClient } from '@tanstack/react-query'
import { Link } from '@tanstack/react-router'
import { Activity as ActivityIcon, Bell, Bot, ListChecks, Network, PanelLeft, Wallet, Workflow } from 'lucide-react'
import { type ReactNode, useEffect, useState } from 'react'
import { Composer } from '@/components/chat/Composer'
import { SecretRequestCard } from '@/components/chat/SecretRequestCard'
import { useSandboxes } from '@/components/settings/sandbox-queries'
import { useRoutines } from '@/components/routines/queries'
import { SearchTrigger } from '@/components/sidebar/SearchTrigger'
import { age } from '@/components/tasks/helpers'
import { useAgents, useMesh, useTasks } from '@/components/tasks/queries'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { useSidebar } from '@/components/ui/sidebar'
import { Skeleton } from '@/components/ui/skeleton'
import { isMacDesktop } from '@/lib/hotkeys'
import { canPopout, openPopout } from '@/lib/popout'
import { cn } from '@/lib/utils'
import type { ActivityEntry } from '@/rpc/bindings/ActivityEntry'
import type { AgentSummary } from '@/rpc/bindings/AgentSummary'
import type { AttentionItem } from '@/rpc/bindings/AttentionItem'
import type { ScheduleInfo } from '@/rpc/bindings/ScheduleInfo'
import type { Task } from '@/rpc/bindings/Task'
import { useSecretRequests } from '@/store/approvals'
import { boardPulse, headline, inFlight, runningRoutines, snoozeUntil, spend } from './model'
import { ATTENTION_KEY, SECRET_REQUESTS_KEY, useActivity, useAttention, useAttentionActions, useSecretRequestRows } from './queries'

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

export const KIND_LABEL: Record<AttentionItem['kind'], string> = {
  approval: 'approval',
  decision: 'decision',
  hire: 'hire',
  blocked: 'blocked',
  review: 'review',
  budget: 'budget',
  node: 'node',
  handoff: 'hand-off',
  secret: 'secret',
  ask: 'question',
  show: 'shown',
  resume: 'resume',
}

/** The inbox's empty state; `/attention`'s rail says the same (101, 104). */
export const ATTENTION_EMPTY = 'Nothing needs you. Agents\u2019 questions and what they show you, tool approvals, decisions agents escalate, hires, blocked or review tasks, budgets at 80 %, unreachable nodes, desktop hand-offs, secret requests and turns a restart could not finish cleanly land here.'

/** The inbox rows, or what will land in them (the HUD's first section). */
export function AttentionList({ items }: { items: AttentionItem[] | undefined }) {
  if (!items || items.length === 0) return <EmptyNote>{ATTENTION_EMPTY}</EmptyNote>
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
  const qc = useQueryClient()
  // 79 §6.6: the VM a secret request or hand-off was raised on (`engine` ↔ `SandboxInfo.engine`) names its origin.
  const sandboxes = useSandboxes()
  // 79 §6.6: the replicated row (`secret_requests_list`) is the card's source on every engine — `title`, `fields`, and
  // the VM it was raised on (`engine` ↔ `SandboxInfo.engine`); the local `SecretRequested` fills in until the poll lands.
  const secretId = item.kind === 'secret' ? item.id.replace(/^secret:/, '') : null
  const rows = useSecretRequestRows(secretId !== null)
  const row = secretId !== null ? rows.data?.find((r) => r.id === secretId) : undefined
  const local = useSecretRequests((s) => (secretId !== null ? s.pending.find((p) => p.id === secretId) : undefined))
  if (row || local) {
    const origin = row ? (sandboxes.data?.find((s) => s.engine === row.engine)?.name ?? item.detail) : item.detail
    const one = row?.fields.length === 1 ? row.fields[0] : undefined
    const answered = () => {
      void qc.invalidateQueries({ queryKey: SECRET_REQUESTS_KEY })
      void qc.invalidateQueries({ queryKey: ATTENTION_KEY })
    }
    return (
      <li className="px-3 py-1 text-[13px]">
        {row ? (
          <SecretRequestCard id={row.id} title={row.title} note={one && one.label !== one.name ? one.label : undefined} fields={row.fields} origin={origin} onAnswered={answered} />
        ) : local ? (
          <SecretRequestCard id={local.id} note={local.note} fields={[{ name: local.name, label: local.name, masked: true, current: local.current }]} origin={origin} onAnswered={answered} />
        ) : null}
      </li>
    )
  }
  return (
    <li className="flex flex-col gap-2 px-3 py-2 text-[13px] sm:flex-row sm:items-center sm:gap-3">
      <div className="min-w-0 flex-1">
        <div className="flex items-center gap-2">
          <span className="rounded-full bg-layer-2 px-2 py-0.5 text-[11px] uppercase tracking-wide text-fg-secondary">{KIND_LABEL[item.kind]}</span>
          {/* 104 A8: every title opens its screen on the attention surface (101 D12 did this for decisions and reviews). */}
          <Link to="/attention/$id" params={{ id: item.id }} className="truncate font-medium hover:underline">
            {item.title}
          </Link>
          <span className="ml-auto shrink-0 text-xs text-fg-tertiary tabular-nums">{age(item.at_ms)}</span>
        </div>
        {item.detail && <p className="truncate text-fg-secondary">{item.detail}</p>}
      </div>
      <KindActions item={item} />
    </li>
  )
}

/**
 * The per-kind buttons of an inbox item — the card's inline block and the plain screen's action row (101 D7) share it.
 * `onResolved` fires after a mutation that removes the item from the list succeeded (101 D11: the screen moves on).
 */
export function KindActions({ item, size = 'xs', onResolved }: { item: AttentionItem; size?: 'xs' | 'sm' | 'default'; onResolved?: () => void }) {
  const act = useAttentionActions()
  const [answer, setAnswer] = useState('')
  const busy = Object.values(act).some((a) => a.isPending)
  const sandboxes = useSandboxes()
  const vm = item.kind === 'handoff' ? sandboxes.data?.find((s) => s.name === item.detail || (s.engine !== null && s.engine === item.detail)) : undefined
  const handoffId = item.kind === 'handoff' ? item.id.replace(/^handoff:/, '') : null
  const rows = useSecretRequestRows(item.kind === 'secret')
  // `useAction` already toasts the failure; the swallowed rejection only keeps the screen where it is.
  const then = (p: Promise<unknown>) => void p.then(() => onResolved?.(), () => undefined)
  return (
    <div className="flex shrink-0 flex-wrap items-center gap-1.5">
      {(item.kind === 'ask' || item.kind === 'show') && (
        // 104: the questions and the artifact need the screen; the card only points there.
        <Button asChild size={size} variant="outline">
          <Link to="/attention/$id" params={{ id: item.id }}>
            {item.kind === 'ask' ? 'Answer' : 'Open'}
          </Link>
        </Button>
      )}
      {item.kind === 'handoff' && handoffId && (
        <>
          {vm && canPopout() && (
            <Button size={size} disabled={busy} onClick={() => void openPopout({ kind: 'vm', name: vm.name })}>
              Open desktop
            </Button>
          )}
          <Button size={size} variant={vm ? 'outline' : 'default'} disabled={busy} onClick={() => then(act.handoff.mutateAsync({ id: handoffId, outcome: 'done' }))}>
            Done
          </Button>
          <Button size={size} variant="outline" disabled={busy} onClick={() => then(act.handoff.mutateAsync({ id: handoffId, outcome: 'cancelled' }))}>
            Cancel
          </Button>
        </>
      )}
      {item.kind === 'resume' && item.session && (
        // 109 §8.3: under `resume: ask` the turn waits for this button; a report item (detail set) only needs reading.
        <>
          {item.detail === null && (
            <Button size={size} disabled={busy} onClick={() => then(act.resume.mutateAsync(item.session as string))}>
              Resume
            </Button>
          )}
          <Button size={size} variant="outline" disabled={busy} onClick={() => then(act.dismiss.mutateAsync(item.id))}>
            Dismiss
          </Button>
        </>
      )}
      {item.kind === 'secret' && <span className="text-xs text-fg-tertiary">{rows.isPending ? 'loading the request…' : `answer it in the session${item.detail ? ` on ${item.detail}` : ''}`}</span>}
      {item.kind === 'approval' && item.approval && (
        <>
          <Button size={size} disabled={busy} onClick={() => then(act.approve.mutateAsync({ id: item.approval as NonNullable<typeof item.approval>, allow: true }))}>
            Allow
          </Button>
          <Button size={size} variant="outline" disabled={busy} onClick={() => then(act.approve.mutateAsync({ id: item.approval as NonNullable<typeof item.approval>, allow: false }))}>
            Deny
          </Button>
        </>
      )}
      {item.kind === 'decision' && item.decision && (
        <form
          className="flex items-center gap-1.5"
          onSubmit={(e) => {
            e.preventDefault()
            if (answer.trim() && item.decision) then(act.decide.mutateAsync({ decision: item.decision, answer: answer.trim() }))
          }}
        >
          <Input value={answer} onChange={(e) => setAnswer(e.target.value)} placeholder="Your decision…" className={cn('w-44 text-base', size === 'xs' ? 'h-7 md:text-xs' : 'h-8 md:text-sm')} aria-label="Decision" />
          <Button size={size} type="submit" disabled={busy || !answer.trim()}>
            Answer
          </Button>
        </form>
      )}
      {item.kind === 'hire' && item.agent && (
        <>
          <Button size={size} disabled={busy} onClick={() => then(act.hire.mutateAsync(item.agent as string))}>
            Hire
          </Button>
          <Button size={size} variant="outline" disabled={busy} onClick={() => then(act.decline.mutateAsync(item.agent as string))}>
            Decline
          </Button>
        </>
      )}
      {item.kind === 'review' && item.task && (
        <>
          <Button size={size} disabled={busy} onClick={() => then(act.move.mutateAsync({ id: item.task as string, state: 'done' }))}>
            Move done
          </Button>
          <Button size={size} variant="outline" disabled={busy} onClick={() => then(act.move.mutateAsync({ id: item.task as string, state: 'doing' }))}>
            Send back
          </Button>
        </>
      )}
      {item.kind === 'blocked' && item.task && (
        <Button size={size} variant="outline" disabled={busy} onClick={() => then(act.move.mutateAsync({ id: item.task as string, state: 'todo' }))}>
          Unblock
        </Button>
      )}
      {item.kind === 'budget' && item.agent && (
        <Button size={size} variant="outline" disabled={busy} onClick={() => then(act.unpause.mutateAsync(item.agent as string))}>
          Un-pause
        </Button>
      )}
      <Button size={size} variant="ghost" disabled={busy} title="Snooze until 09:00 tomorrow" onClick={() => then(act.snooze.mutateAsync({ id: item.id, until: snoozeUntil('tomorrow') }))}>
        Snooze
      </Button>
      <Button size={size} variant="ghost" disabled={busy} onClick={() => then(act.dismiss.mutateAsync(item.id))}>
        Dismiss
      </Button>
    </div>
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
