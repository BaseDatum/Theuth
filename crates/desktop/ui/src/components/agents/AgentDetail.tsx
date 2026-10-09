import { useQuery } from '@tanstack/react-query'
import { ModelCombobox } from '@/components/chat/ModelCombobox'
import { SAME_AS_NEW } from '@/components/settings/Chat'
import { useAction } from '@/components/settings/shared'
import { Link, useNavigate } from '@tanstack/react-router'
import { ExternalLink, Trash2 } from 'lucide-react'
import { useState } from 'react'
import { SectionBand } from '@/components/nav/SectionBand'
import { SectionRow } from '@/components/nav/SectionRow'
import { TaskCard } from '@/components/tasks/Board'
import { THEUTH, age, columns } from '@/components/tasks/helpers'
import { useAgents, useTasks } from '@/components/tasks/queries'
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from '@/components/ui/alert-dialog'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Tabs, TabsList, TabsTrigger } from '@/components/ui/tabs'
import { queryKeys, useApi } from '@/lib/rpc'
import { byRank, rankSummary, rootOf } from '@/lib/session-tree'
import { cn } from '@/lib/utils'
import type { AgentFile } from '@/rpc/bindings/AgentFile'
import type { AgentSummary } from '@/rpc/bindings/AgentSummary'
import { CharterDoc } from './CharterDoc'
import { CharterEditor } from './CharterEditor'
import { AccessFields, Field, RuntimeFields } from './RuntimeAccessFields'
import { ActiveSwitch, ActivityChip, AgentAvatar, BudgetBar } from './identity'
import { useAgentActions, useAgentFile } from './queries'
import { budgetUsed } from './tree'

export const TABS = ['overview', 'charter', 'runtime', 'runs', 'tasks'] as const
export type Tab = (typeof TABS)[number]
const TAB_LABEL: Record<Tab, string> = { overview: 'Overview', charter: 'Charter', runtime: 'Runtime & Access', runs: 'Runs', tasks: 'Tasks' }

/**
 * `/agents/$name/$tab` — paperclip's AgentDetail: header (avatar, title, Active/Paused switch, activity, actions),
 * then Overview · Charter · Runtime & Access · Runs · Tasks. `theuth` is the built-in root: no switch, no remove,
 * charter read-only (it is `prompt.md`). Activity (a journal feed) has no RPC yet and is omitted.
 */
export function AgentDetail({ name, tab }: { name: string; tab: Tab }) {
  const navigate = useNavigate()
  const agents = useAgents()
  const file = useAgentFile(name)
  const summary = agents.data?.find((a) => a.file.name === name)
  const { approve, reject, remove } = useAgentActions()
  const [confirmRemove, setConfirmRemove] = useState(false)
  const root = name === THEUTH
  const proposed = summary?.status === 'proposed' || !!file.data?.proposed_by

  return (
    <article className="flex min-h-0 min-w-0 flex-1 flex-col">
      <SectionBand group="agents" />
      <SectionRow
        back={{ to: '/agents', label: 'Back to agents' }}
        title={
          <>
            <span className="text-fg-tertiary">Agents › </span>
            {file.data?.title ?? name}
          </>
        }
      />
      {file.isPending && <p className="p-4 text-sm text-fg-tertiary">Loading…</p>}
      {file.isError && <p className="p-4 text-sm text-fg-tertiary">{file.error.message}</p>}
      {file.data && (
        <>
          <div className="flex flex-wrap items-center gap-3 border-b border-border-subtle px-4 py-3">
            <AgentAvatar name={name} title={file.data.title} size={48} />
            <div className="min-w-0 flex-1">
              <div className="flex flex-wrap items-center gap-2">
                <h3 className="text-base font-semibold">{file.data.title}</h3>
                <span className="font-mono text-xs text-fg-tertiary">{name}</span>
                {root && <Badge variant="outline">built-in</Badge>}
              </div>
              <div className="mt-1 flex flex-wrap items-center gap-2 text-xs text-fg-secondary">
                {summary && !proposed && <ActiveSwitch agent={summary} />}
                {summary && !summary.file.paused && !proposed && <ActivityChip status={summary.status} />}
                {proposed && <Badge>proposed by {file.data.proposed_by ?? '—'}</Badge>}
                <span>reports to {root ? 'you' : file.data.reports_to}</span>
                {summary?.error && <span className="text-danger">{summary.error}</span>}
              </div>
            </div>
            {proposed ? (
              <>
                <Button size="sm" disabled={approve.isPending} onClick={() => approve.mutate(name)}>
                  Approve hire
                </Button>
                <Button
                  size="sm"
                  variant="outline"
                  disabled={reject.isPending}
                  onClick={() => reject.mutate({ name, note: '' }, { onSuccess: () => void navigate({ to: '/agents' }) })}
                >
                  Decline
                </Button>
              </>
            ) : (
              <>
                {summary?.desk && (
                  <Button asChild size="sm" variant="outline">
                    <Link to="/c/$session" params={{ session: summary.desk }}>
                      <ExternalLink /> Open desk
                    </Link>
                  </Button>
                )}
                {!root && (
                  <Button size="sm" variant="ghost" aria-label="Remove agent" onClick={() => setConfirmRemove(true)}>
                    <Trash2 /> Remove
                  </Button>
                )}
              </>
            )}
          </div>
          <Tabs value={tab} onValueChange={(t) => void navigate({ to: '/agents/$name/$tab', params: { name, tab: t }, replace: true })} className="min-h-0 flex-1 gap-0">
            <TabsList variant="line" className="w-full justify-start overflow-x-auto border-b border-border-subtle px-2">
              {TABS.map((t) => (
                <TabsTrigger key={t} value={t}>
                  {TAB_LABEL[t]}
                </TabsTrigger>
              ))}
            </TabsList>
            <div className="min-h-0 flex-1 overflow-y-auto">
              {tab === 'overview' && <Overview file={file.data} summary={summary} agents={agents.data ?? []} />}
              {tab === 'charter' && <Charter file={file.data} />}
              {tab === 'runtime' && <Runtime key={JSON.stringify(file.data)} file={file.data} />}
              {tab === 'runs' && <Runs name={name} desk={summary?.desk ?? null} />}
              {tab === 'tasks' && <TasksTab name={name} />}
            </div>
          </Tabs>
        </>
      )}
      <AlertDialog open={confirmRemove} onOpenChange={setConfirmRemove}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>Remove {file.data?.title}?</AlertDialogTitle>
            <AlertDialogDescription>
              Deletes <code>agents/{name}.md</code>. Its desk closes; its tasks stay on the board unowned until you reassign them. Agents reporting to it move under theuth.
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>Keep</AlertDialogCancel>
            <AlertDialogAction variant="destructive" onClick={() => remove.mutate(name, { onSuccess: () => void navigate({ to: '/agents' }) })}>
              Remove
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </article>
  )
}

function Card({ title, action, children }: { title: string; action?: React.ReactNode; children: React.ReactNode }) {
  return (
    <section className="rounded-lg border border-border-subtle p-3 text-sm">
      <h4 className="mb-2 flex items-center justify-between text-xs font-semibold uppercase tracking-wide text-fg-secondary">
        {title}
        {action}
      </h4>
      {children}
    </section>
  )
}

function Rows({ rows }: { rows: [string, React.ReactNode][] }) {
  return (
    <dl className="grid grid-cols-[auto_1fr] gap-x-4 gap-y-1.5">
      {rows.map(([k, v]) => (
        <div key={k} className="contents">
          <dt className="text-fg-tertiary">{k}</dt>
          <dd className="min-w-0 truncate">{v}</dd>
        </div>
      ))}
    </dl>
  )
}

const dim = (s: string) => <span className="text-fg-tertiary">{s}</span>

function Overview({ file: f, summary: s, agents }: { file: AgentFile; summary: AgentSummary | undefined; agents: AgentSummary[] }) {
  const reports = agents.filter((a) => a.file.reports_to === f.name && a.file.name !== f.name)
  const open = useTasks({ owner: `agent:${f.name}`, state: ['todo', 'doing', 'blocked', 'review'], engine: null, project: null })
  const root = f.name === THEUTH
  return (
    <div className="grid gap-3 p-4 md:grid-cols-2">
      <Card title="Identity">
        <Rows
          rows={[
            ['Title', f.title],
            ['Reports to', root ? 'you' : <Link to="/agents/$name/$tab" params={{ name: f.reports_to, tab: 'overview' }} className="text-primary hover:underline">{f.reports_to}</Link>],
            [
              'Direct reports',
              reports.length === 0 ? (
                dim('none')
              ) : (
                <span className="flex flex-wrap gap-1">
                  {reports.map((r) => (
                    <Link key={r.file.name} to="/agents/$name/$tab" params={{ name: r.file.name, tab: 'overview' }} className="rounded-full border border-border-subtle px-2 text-xs hover:bg-layer-2">
                      {r.file.title}
                    </Link>
                  ))}
                </span>
              ),
            ],
          ]}
        />
        <p className="mt-3 text-[13px] text-fg-secondary">{f.capabilities?.trim() || dim(root ? 'Answers anything unaddressed, owns delegation, root of every reporting line.' : 'No capability summary has been added.')}</p>
      </Card>
      <Card
        title="Runtime & Access"
        action={
          !root && (
            <Link to="/agents/$name/$tab" params={{ name: f.name, tab: 'runtime' }} className="font-normal normal-case text-primary hover:underline">
              Configure
            </Link>
          )
        }
      >
        <Rows
          rows={[
            ['Model', f.model ?? dim(SAME_AS_NEW.label)],
            ['Effort', f.effort ?? dim('default')],
            ['Tools ceiling', f.tools ?? dim('inherited')],
            ['MCP', f.mcp === null ? dim('every server') : f.mcp.length === 0 ? dim('none') : f.mcp.join(', ')],
            ['Desk', s?.desk ? <Link to="/c/$session" params={{ session: s.desk }} className="text-primary hover:underline">open session</Link> : dim('no desk open')],
          ]}
        />
      </Card>
      <Card
        title="Work"
        action={
          <Link to="/agents/$name/$tab" params={{ name: f.name, tab: 'tasks' }} className="font-normal normal-case text-primary hover:underline">
            See all
          </Link>
        }
      >
        <p className="mb-2 text-[13px] text-fg-secondary">
          <b className="tabular-nums text-foreground">{s?.open_tasks ?? open.data?.length ?? 0}</b> open tasks
        </p>
        {open.data && open.data.length === 0 && <p className="text-[13px] text-fg-tertiary">No open tasks.</p>}
        <ul className="flex flex-col gap-1">
          {open.data?.slice(0, 6).map((t) => (
            <li key={t.id}>
              <Link to="/tasks/$id" params={{ id: t.id }} className="flex items-center gap-2 rounded-md px-1.5 py-1 text-[13px] hover:bg-layer-2">
                <span className="min-w-0 flex-1 truncate">{t.title}</span>
                <span className="text-xs text-fg-tertiary">{t.state}</span>
              </Link>
            </li>
          ))}
        </ul>
      </Card>
      {s && <BudgetCard file={f} summary={s} />}
    </div>
  )
}

/** Paperclip's BudgetPolicyCard over a theuth file: status pill, observed / budget / remaining, set or change the cap. */
function BudgetCard({ file: f, summary: s }: { file: AgentFile; summary: AgentSummary }) {
  const { put } = useAgentActions()
  const [editing, setEditing] = useState(false)
  const [amount, setAmount] = useState(f.budget.type === 'monthly' ? String(f.budget.amount) : '')
  const [currency, setCurrency] = useState(f.budget.type === 'monthly' ? f.budget.currency : 'USD')
  const used = budgetUsed(s)
  const root = f.name === THEUTH
  const state = f.budget.type === 'none' ? 'none' : f.paused && used !== null && used >= 1 ? 'paused' : used !== null && used >= 1 ? 'hard_stop' : used !== null && used >= 0.8 ? 'warning' : 'ok'
  const pill: Record<typeof state, [string, string]> = {
    none: ['No cap', 'border-border text-fg-tertiary'],
    ok: ['Healthy', 'border-success/40 text-success'],
    warning: ['Warning', 'border-warning/40 text-warning'],
    hard_stop: ['Hard stop', 'border-danger/40 text-danger'],
    paused: ['Paused by budget', 'border-danger/40 text-danger'],
  }
  const n = Number(amount)
  const valid = n > 0 && /^[A-Z]{3}$/.test(currency)
  return (
    <Card title="Budget" action={<span className={cn('rounded-full border px-2 py-0.5 font-medium normal-case', pill[state][1])}>{pill[state][0]}</span>}>
      <p className="mb-2 text-xs text-fg-tertiary">Monthly, calendar month · billed model spend across every session it opens.</p>
      {f.budget.type === 'monthly' ? (
        <>
          <div className="grid grid-cols-3 gap-2 text-[13px]">
            <div>
              <div className="text-xs text-fg-tertiary">Observed</div>
              <div className="tabular-nums">
                {s.spent_month.toFixed(2)} <span className="text-fg-tertiary">{used !== null && `· ${Math.round(used * 100)}% of limit`}</span>
              </div>
            </div>
            <div>
              <div className="text-xs text-fg-tertiary">Budget</div>
              <div className="tabular-nums">
                {f.budget.amount} {f.budget.currency} <span className="text-fg-tertiary">· soft alert at 80%</span>
              </div>
            </div>
            <div>
              <div className="text-xs text-fg-tertiary">Remaining</div>
              <div className="tabular-nums">{Math.max(0, f.budget.amount - s.spent_month).toFixed(2)}</div>
            </div>
          </div>
          <BudgetBar agent={s} className="mt-2" />
          {state === 'paused' && (
            <p className="mt-2 rounded-md bg-danger/10 p-2 text-[13px] text-danger">
              Wakes are held until you switch it back to Active — that lifts the cap for the rest of the month — or raise the budget.
            </p>
          )}
          {state === 'hard_stop' && !f.paused && <p className="mt-2 text-[13px] text-warning">Over the cap; the next spend check pauses it.</p>}
        </>
      ) : (
        <p className="text-[13px] text-fg-secondary">{root ? 'The built-in root is never capped.' : 'No cap configured. Spend is counted but never stops this agent.'}</p>
      )}
      {!root &&
        (editing ? (
          <form
            className="mt-3 flex flex-wrap items-end gap-2"
            onSubmit={(e) => {
              e.preventDefault()
              if (!valid) return
              put.mutate({ ...f, budget: { type: 'monthly', amount: n, currency } }, { onSuccess: () => setEditing(false) })
            }}
          >
            <Field label="Budget per month" id="bc-amount">
              <Input id="bc-amount" type="number" min={0} step="any" className="w-28" value={amount} onChange={(e) => setAmount(e.target.value)} placeholder="0.00" />
            </Field>
            <Input className="w-20 uppercase" aria-label="Currency" maxLength={3} value={currency} onChange={(e) => setCurrency(e.target.value.toUpperCase())} />
            <Button type="submit" size="sm" disabled={!valid || put.isPending}>
              {f.budget.type === 'monthly' ? 'Update budget' : 'Set budget'}
            </Button>
            {f.budget.type === 'monthly' && (
              <Button type="button" size="sm" variant="ghost" disabled={put.isPending} onClick={() => put.mutate({ ...f, budget: { type: 'none' } }, { onSuccess: () => setEditing(false) })}>
                Remove cap
              </Button>
            )}
            <Button type="button" size="sm" variant="ghost" onClick={() => setEditing(false)}>
              Cancel
            </Button>
            {!valid && amount !== '' && <p className="w-full text-xs text-danger">Enter a valid amount above zero and a three-letter currency.</p>}
          </form>
        ) : (
          <Button size="sm" variant="outline" className="mt-3" onClick={() => setEditing(true)}>
            {f.budget.type === 'monthly' ? 'Change budget' : 'Set budget'}
          </Button>
        ))}
    </Card>
  )
}

/** The charter document with its chat (`CharterDoc`); theuth's `prompt.md` stays a read-only view. */
function Charter({ file: f }: { file: AgentFile }) {
  if (f.name !== THEUTH) {
    return <CharterDoc who={f} hint={`The desk's system prompt is built from this; edits land in agents/${f.name}.md once you pause.`} className="h-full" />
  }
  return (
    <div className="flex h-full min-h-0 flex-col">
      <p className="border-b border-border-subtle px-4 py-2 text-[13px] text-fg-secondary">
        theuth's charter is <code>prompt.md</code> in the data directory, read-only here.
      </p>
      <div className="min-h-0 flex-1 p-4">
        <CharterEditor value={f.body} readOnly className="h-full min-h-[320px]" />
      </div>
    </div>
  )
}

/** 148 R3: theuth's model (`roster.theuth_model`) — the same picker, RPC and query as Settings › Chat; the roster refetches so the header's model updates. */
function TheuthRuntime() {
  const api = useApi()
  const models = useQuery({ queryKey: queryKeys.models, queryFn: () => api.list_models() })
  const model = useQuery({ queryKey: queryKeys.theuthAgentModel, queryFn: () => api.theuth_agent_model() })
  const setModel = useAction((id: string | null) => api.set_theuth_agent_model(id), { keys: [queryKeys.theuthAgentModel, queryKeys.roster] })
  return (
    <div className="flex flex-col gap-4 p-4 text-sm text-fg-secondary">
      <Field id="theuth-model" label="Model" hint="Its desk and the work it starts for tasks; subagents below those use the subagent default.">
        <ModelCombobox
          id="theuth-model"
          className="max-w-64"
          models={models.data ?? []}
          value={model.data?.id}
          none={SAME_AS_NEW}
          disabled={model.isPending || setModel.isPending}
          onChange={(v) => setModel.mutate(v)}
        />
      </Field>
      <p>
        The built-in root runs with tools <code>destructive</code>, every MCP server and no budget; only its model is configured here (or in Settings › Chat).
      </p>
    </div>
  )
}

function Runtime({ file: f }: { file: AgentFile }) {
  const { put } = useAgentActions()
  const [d, setD] = useState(f)
  const dirty = JSON.stringify(d) !== JSON.stringify(f)
  if (f.name === THEUTH) return <TheuthRuntime />
  return (
    <div className="flex h-full min-h-0 flex-col">
      <div className="flex flex-col gap-8 p-4">
        <section>
          <h4 className="mb-3 text-sm font-semibold">Runtime</h4>
          <RuntimeFields d={d} set={(k, v) => setD((x) => ({ ...x, [k]: v }))} />
        </section>
        <section>
          <h4 className="mb-3 text-sm font-semibold">Access</h4>
          <AccessFields d={d} set={(k, v) => setD((x) => ({ ...x, [k]: v }))} />
        </section>
      </div>
      <footer className="sticky bottom-0 flex shrink-0 items-center gap-2 border-t border-border-subtle bg-background px-4 py-2">
        <span className="flex-1 text-[13px] text-fg-tertiary">{dirty && 'You have unsaved changes.'}</span>
        <Button variant="ghost" size="sm" disabled={!dirty || put.isPending} onClick={() => setD(f)}>
          Discard
        </Button>
        <Button size="sm" disabled={!dirty || put.isPending} onClick={() => put.mutate(d)}>
          Save changes
        </Button>
      </footer>
    </div>
  )
}

/** The agent's sessions under the one list rule (running first, then most recently active): its desks (the row's `desk` mark) and everything under them at any depth (the work sessions and their subagents). */
function Runs({ name, desk }: { name: string; desk: string | null }) {
  const api = useApi()
  const sessions = useQuery({ queryKey: queryKeys.sessions, queryFn: () => api.list_sessions() })
  const all = sessions.data ?? []
  const desks = all.filter((s) => s.desk === name || s.id === desk).map((s) => s.id)
  const runs = all.filter((s) => desks.includes(rootOf(all, s.id) ?? s.id)).sort((a, b) => byRank(rankSummary(a), rankSummary(b)))
  if (sessions.isPending) return <p className="p-4 text-sm text-fg-tertiary">Loading…</p>
  if (runs.length === 0) return <p className="p-4 text-sm text-fg-tertiary">No runs yet. A desk opens on the first wake addressed to this agent.</p>
  return (
    <ul className="divide-y divide-border-subtle">
      {runs.map((s) => (
        <li key={s.id}>
          <Link to="/c/$session" params={{ session: s.id }} className="flex items-center gap-3 px-4 py-2 text-sm hover:bg-layer-2">
            <span className={cn('size-2 shrink-0 rounded-full', s.running ? 'bg-success' : 'bg-fg-tertiary/40')} aria-hidden />
            <span className="min-w-0 flex-1 truncate">
              {s.title || 'untitled'} {desks.includes(s.id) && <Badge variant="outline">desk</Badge>}
            </span>
            <span className="hidden truncate text-xs text-fg-tertiary sm:inline">{s.model}</span>
            <span className="text-xs text-fg-tertiary tabular-nums">{age(Date.parse(s.updated_at))}</span>
          </Link>
        </li>
      ))}
    </ul>
  )
}

/** The board filtered to this owner, as read-only columns; the board proper takes moves. */
function TasksTab({ name }: { name: string }) {
  const tasks = useTasks({ owner: `agent:${name}`, state: null, engine: null, project: null })
  if (tasks.isPending) return <p className="p-4 text-sm text-fg-tertiary">Loading…</p>
  if (tasks.isError) return <p className="p-4 text-sm text-fg-tertiary">{tasks.error.message}</p>
  return (
    <div className="flex h-full min-h-0 flex-col">
      <div className="flex items-center gap-2 px-4 py-2 text-[13px] text-fg-secondary">
        <span className="flex-1 tabular-nums">{tasks.data.length} tasks</span>
        <Link to="/tasks" search={{ agent: `agent:${name}`, project: '' }} className="text-primary hover:underline">
          Open on the board
        </Link>
      </div>
      <div className="min-h-0 flex-1 overflow-x-auto">
        <div className="flex h-full snap-x gap-3 px-4 pb-4">
          {columns(tasks.data, false)
            .filter((c) => c.tasks.length > 0)
            .map(({ state, tasks: col }) => (
              <section key={state} className="flex h-full w-64 shrink-0 snap-start flex-col rounded-[10px] bg-layer-1">
                <h3 className="flex items-center gap-2 px-3 py-2 text-xs font-semibold uppercase tracking-wide text-fg-secondary">
                  {state}
                  <span className="font-normal text-fg-tertiary tabular-nums">{col.length}</span>
                </h3>
                <ul className="flex min-h-0 flex-1 flex-col gap-2 overflow-y-auto px-2 pb-2">
                  {col.map((t) => (
                    <TaskCard key={t.id} task={t} />
                  ))}
                </ul>
              </section>
            ))}
          {tasks.data.length === 0 && <p className="text-sm text-fg-tertiary">Nothing owned by {name}.</p>}
        </div>
      </div>
    </div>
  )
}
