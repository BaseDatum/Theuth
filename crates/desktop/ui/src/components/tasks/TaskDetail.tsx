import { useQuerySupport } from '@/rpc/capabilities'
import { useMutation, useQueries } from '@tanstack/react-query'
import { Link, useNavigate } from '@tanstack/react-router'
import { MessageSquare, TriangleAlert } from 'lucide-react'
import { type ReactNode, useEffect, useMemo, useState } from 'react'
import { notify as toast } from '@/lib/notify'
import { Streamdown } from 'streamdown'
import { ProjectPicker } from '@/components/chat/ProjectPicker'
import { Frame, Live } from '@/components/editor/DocumentPage'
import { useDocument } from '@/components/editor/useDocument'
import { errorText } from '@/components/settings/shared'
import { ProjectIcon } from '@/components/sidebar/ProjectStyle'
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
import { Skeleton } from '@/components/ui/skeleton'
import { Textarea } from '@/components/ui/textarea'
import { costShort } from '@/lib/money'
import { queryKeys, useApi } from '@/lib/rpc'
import { useConnection } from '@/store/connection'
import { cn } from '@/lib/utils'
import type { Task } from "@/rpc/bindings/method.roster.output"
import type { TaskEvent } from "@/rpc/bindings/method.roster.output"
import type { TaskPatch } from '@/rpc/bindings/method.roster.input'
import { AttachmentsEditor } from './Attachments'
import { rememberOpenedTask } from './boardMemory'
import { MoveButton, TaskMenu } from './MoveHome'
import { OwnerSelect } from './OwnerSelect'
import { TaskChips } from './TaskChips'
import { askSecs, engineName, homeLine } from './home'
import { actionsFor, age, ownerLabel, ownerText, parseBlocker, parseOwner, shortId, stateChip, wakePhrase } from './helpers'
import { doneBecause, useAgents, useCloseDone, useInvalidateTasks, useMesh, useMoveTask, useTask, useTaskEvents, useTasks } from './queries'

/**
 * `/tasks/$id` (67 §12, 69): brief, blocked-by and blocks (editable chips), placement, the thread document with a
 * comment box, sessions, cost, the field-level event log, and the actions the state allows. Every action is one RPC;
 * errors toast verbatim. Links live in the brief; the record's `external` field is not shown.
 */
export function TaskDetail({ id }: { id: string }) {
  const task = useTask(id)
  const events = useTaskEvents(id)
  const mesh = useMesh()
  const info = useConnection((s) => s.info)
  const [confirmCancel, setConfirmCancel] = useState(false)
  // 118: the board scrolls this card into view and rings it when the user comes back.
  useEffect(() => rememberOpenedTask(id), [id])
  if (task.isPending) {
    return (
      <Frame backTo="/tasks" backLabel="Back to tasks" backAlways title={<Skeleton className="h-5 w-48" />}>
        <div className="space-y-3 p-6">
          <Skeleton className="h-4 w-2/3" />
          <Skeleton className="h-4 w-1/2" />
        </div>
      </Frame>
    )
  }
  if (task.isError) {
    return (
      <Frame backTo="/tasks" backLabel="Back to tasks" backAlways title={<h2 className="text-sm font-semibold">Task</h2>}>
        <p className="p-6 text-center text-sm text-fg-tertiary">{task.error.message}</p>
      </Frame>
    )
  }
  const t = task.data
  const home = homeLine(t, mesh.data, askSecs(info))
  return (
    <Frame
      backTo="/tasks"
      backLabel="Back to tasks"
      backAlways
      actions={
        <TaskMenu
          task={t}
          onCancel={actionsFor(t).cancel ? () => setConfirmCancel(true) : null}
        />
      }
      title={
        <h2 className="flex min-w-0 flex-1 items-center gap-2 truncate text-sm font-semibold">
          <ProjectIcon project={t.project} className="size-4" />
          <span className="truncate">{t.title}</span>
          <StateBadge task={t} />
        </h2>
      }
    >
      <div className="min-h-0 flex-1 overflow-y-auto">
        <div className="mx-auto flex w-full max-w-[1100px] flex-col gap-6 p-4 md:flex-row">
          <div className="flex min-w-0 flex-1 flex-col gap-6">
            {home && (
              <div className="text-sm">
                <p>
                  {home.text}
                  {home.offline && (
                    <span className="inline-flex items-center gap-1.5 font-medium text-warning">
                      <span className="mx-1 font-normal text-fg-tertiary">·</span>
                      <span className="size-1.5 rounded-full bg-fg-tertiary" aria-hidden />
                      {home.offline}
                    </span>
                  )}
                </p>
                {home.callout && (
                  <div role="alert" className="mt-2 flex flex-col items-start gap-2 rounded-xl border border-warning-border/40 bg-warning-bg p-3">
                    <p className="flex items-start gap-2 text-[13px]">
                      <TriangleAlert className="mt-0.5 size-4 shrink-0 text-warning" aria-hidden />
                      {home.callout}
                    </p>
                    <MoveButton task={t} />
                  </div>
                )}
              </div>
            )}
            <Section title="Brief">
              <Streamdown mode="static" className="text-sm">
                {t.brief || '_No brief._'}
              </Streamdown>
            </Section>
            <Attachments task={t} />
            <Relations task={t} />
            <Placement task={t} events={events.data ?? []} />
            <Section title="Thread">
              <Thread task={t} />
            </Section>
            <Section title="Events">
              {events.isError && <p className="text-sm text-fg-tertiary">{events.error.message}</p>}
              {events.data?.length === 0 && <p className="text-sm text-fg-tertiary">No events yet.</p>}
              <ol className="space-y-1.5 text-sm">
                {events.data?.map((e, i) => (
                  <li key={i} className="flex flex-wrap items-baseline gap-x-2">
                    <span className="w-12 shrink-0 text-xs text-fg-tertiary tabular-nums">{age(e.at_ms)}</span>
                    <span className="text-fg-secondary">{e.actor}</span>
                    <span className="font-medium">{e.field}</span>
                    {e.field === 'comment' || e.before === null ? (
                      <span className={cn('wrap-anywhere', e.field !== 'comment' && 'text-fg-secondary')}>{e.after}</span>
                    ) : (
                      <span className="wrap-anywhere text-fg-secondary">
                        {e.before} → {e.after ?? '—'}
                      </span>
                    )}
                  </li>
                ))}
              </ol>
            </Section>
          </div>
          <aside className="flex w-full shrink-0 flex-col gap-6 md:w-72">
            <Actions task={t} confirmCancel={confirmCancel} setConfirmCancel={setConfirmCancel} />
            <Sessions task={t} events={events.data ?? []} />
            <Section title="Record">
              <dl className="grid grid-cols-[auto_1fr] gap-x-3 gap-y-1 text-[13px]">
                <dt className="text-fg-tertiary">id</dt>
                <dd className="truncate font-mono">{t.id}</dd>
                <dt className="text-fg-tertiary">created</dt>
                <dd>
                  {age(t.created_ms)} ago by {t.created_by}
                </dd>
                {t.parent && (
                  <>
                    <dt className="text-fg-tertiary">parent</dt>
                    <dd>
                      <Link to="/tasks/$id" params={{ id: t.parent }} className="text-primary hover:underline">
                        task {shortId(t.parent)}
                      </Link>
                    </dd>
                  </>
                )}
                {t.schedule && (
                  <>
                    <dt className="text-fg-tertiary">schedule</dt>
                    <dd>{t.schedule[0]}</dd>
                  </>
                )}
                {t.no_progress_count > 0 && (
                  <>
                    <dt className="text-fg-tertiary">no progress</dt>
                    <dd>{t.no_progress_count} wakes</dd>
                  </>
                )}
              </dl>
            </Section>
          </aside>
        </div>
      </div>
    </Frame>
  )
}

/** The state chip; while `doing`, "at ops's desk · open session" links to the checkout session on its node. */
function StateBadge({ task: t }: { task: Task }) {
  const mesh = useMesh().data
  const text = stateChip(t)
  if (t.state === 'doing' && t.checkout) {
    return (
      <Link to="/c/$session" params={{ session: t.checkout.session }} title={`open the session on ${engineName(mesh, t.checkout.engine)}`}>
        <Badge className="hover:opacity-80">
          {text} · {engineName(mesh, t.checkout.engine)}
        </Badge>
      </Link>
    )
  }
  return <Badge variant="outline">{text}</Badge>
}

/** Chips + picker over `task_update {attachments}` (replace whole); each change is an `attached`/`detached` event. */
function Attachments({ task: t }: { task: Task }) {
  const api = useApi()
  const invalidate = useInvalidateTasks()
  const patch = useMutation({
    mutationFn: (attachments: Task['attachments']) => api.task_update(t.id, { attachments }),
    onSuccess: () => invalidate(t.id),
    onError: (e) => toast.error(errorText(e)),
  })
  return (
    <Section title={t.attachments.length ? `Attachments ${t.attachments.length}` : 'Attachments'}>
      <AttachmentsEditor value={t.attachments} onChange={(next) => patch.mutate(next)} disabled={patch.isPending} />
    </Section>
  )
}

/**
 * *Blocked by* and *Blocks* side by side (69): task blockers as editable chips (each opens the other task), decision
 * and person blockers as text; *Blocks* edits the other task's `blockers`. The inverse is computed by the engine.
 */
function Relations({ task: t }: { task: Task }) {
  const api = useApi()
  const invalidate = useInvalidateTasks()
  const all = useTasks()
  const byId = Object.fromEntries((all.data ?? []).map((x) => [x.id, x]))
  const taskBlockers = t.blockers.filter((raw) => parseBlocker(raw).kind === 'task')
  const otherBlockers = t.blockers.filter((raw) => parseBlocker(raw).kind !== 'task')
  const blockedBy = taskBlockers.flatMap((id) => (byId[id] ? [byId[id]] : []))
  const blocks = t.blocks.flatMap((id) => (byId[id] ? [byId[id]] : []))
  const patch = useMutation({
    mutationFn: ({ id, blockers }: { id: string; blockers: string[] }) => api.task_update(id, { blockers }),
    onSuccess: () => invalidate(t.id),
    onError: (e) => toast.error(errorText(e)),
  })
  const busy = patch.isPending
  return (
    <div className="grid gap-6 sm:grid-cols-2">
      <Section title={`Blocked by ${t.blockers.length}`}>
        <TaskChips
          title="Blocked by"
          tasks={blockedBy}
          project={t.project}
          exclude={[t.id, ...t.blocks]}
          disabled={busy}
          onAdd={(b) => patch.mutate({ id: t.id, blockers: [...t.blockers, b.id] })}
          onRemove={(b) => patch.mutate({ id: t.id, blockers: t.blockers.filter((x) => x !== b.id) })}
        />
        {otherBlockers.length > 0 && (
          <ul className="mt-2 space-y-1 text-sm">
            {otherBlockers.map((raw) => (
              <li key={raw} className="font-mono text-fg-secondary">
                {raw}
              </li>
            ))}
          </ul>
        )}
      </Section>
      <Section title={`Blocks ${t.blocks.length}`}>
        <TaskChips
          title="Blocks"
          tasks={blocks}
          project={t.project}
          exclude={[t.id, ...taskBlockers]}
          disabled={busy}
          onAdd={(b) => patch.mutate({ id: b.id, blockers: [...b.blockers, t.id] })}
          onRemove={(b) => patch.mutate({ id: b.id, blockers: b.blockers.filter((x) => x !== t.id) })}
        />
      </Section>
    </div>
  )
}

function Section({ title, children }: { title: string; children: ReactNode }) {
  return (
    <section>
      <h3 className="mb-2 text-xs font-semibold uppercase tracking-wide text-fg-secondary">{title}</h3>
      {children}
    </section>
  )
}

/** `where` / `harness` and the reason theuth gave — the newest `where`/`placement` event's text until the thread block is parsed. */
function Placement({ task: t, events }: { task: Task; events: TaskEvent[] }) {
  const mesh = useMesh().data
  const reason = [...events].reverse().find((e) => e.field === 'placement' || e.field === 'where')
  return (
    <Section title="Placement">
      <div className="flex flex-wrap items-center gap-1.5 text-sm">
        {t.where ? <Badge variant="outline">{t.where}</Badge> : <span className="text-fg-tertiary">not placed yet</span>}
        {t.harness && <Badge variant="outline">{t.harness}</Badge>}
        {t.checkout && (
          <span className="text-fg-secondary">
            · checked out on {engineName(mesh, t.checkout.engine)} (epoch {t.checkout.epoch}), progress {age(t.checkout.progress_at_ms)} ago
          </span>
        )}
      </div>
      {reason && (
        <p className="mt-1 text-[13px] text-fg-secondary">
          {reason.actor}: {reason.after ?? '—'}
        </p>
      )}
    </Section>
  )
}

/**
 * The 59 document behind the task, read-only when the harness says so, plus a comment box → `task_comment`. A comment
 * wakes an agent owner; the user owning the task is told there is nobody to wake.
 */
function Thread({ task: t }: { task: Task }) {
  const api = useApi()
  const invalidate = useInvalidateTasks()
  const [text, setText] = useState('')
  const agents = useAgents()
  const comment = useMutation({
    mutationFn: () => api.task_comment(t.id, text.trim()),
    onSuccess: () => {
      setText('')
      invalidate(t.id)
    },
    onError: (e) => toast.error(errorText(e)),
  })
  return (
    <div className="flex flex-col gap-3">
      {t.thread ? <ThreadDoc id={t.thread} /> : <p className="text-sm text-fg-tertiary">No thread document yet — theuth creates it on the first note.</p>}
      <form
        className="flex flex-col gap-2"
        onSubmit={(e) => {
          e.preventDefault()
          if (text.trim()) comment.mutate()
        }}
      >
        <Textarea rows={2} placeholder={t.owner.type === 'agent' ? `Comment — ${wakePhrase(t.owner.name, agents.data)}` : 'Comment'} value={text} onChange={(e) => setText(e.target.value)} />
        {t.owner.type === 'user' && (
          <Badge variant="outline" className="self-start">
            owner is you — no wake
          </Badge>
        )}
        <Button type="submit" size="sm" className="self-end" disabled={comment.isPending || !text.trim()}>
          <MessageSquare /> Comment
        </Button>
      </form>
    </div>
  )
}

function ThreadDoc({ id }: { id: string }) {
  const live = useDocument(useMemo(() => ({ doc: id }), [id]))
  return (
    <div className="max-h-[480px] overflow-y-auto rounded-lg border border-border-subtle">
      <Live {...live} dense />
    </div>
  )
}

/** Sessions known to the record: the checkout plus every session an event names; each opens in the chat pane. */
function Sessions({ task: t, events }: { task: Task; events: TaskEvent[] }) {
  const canQuery1 = useQuerySupport(["session_cost"])
  const mesh = useMesh().data
  const api = useApi()
  const navigate = useNavigate()
  const ids = [...new Set([t.checkout?.session, ...events.map((e) => e.session)].filter((s): s is string => !!s))]
  const costs = useQueries({
    queries: ids.map((s) => ({ enabled: canQuery1, queryKey: queryKeys.sessionCost(s), queryFn: () => api.session_cost(s), retry: false })),
  })
  return (
    <>
      <Section title="Sessions">
        {ids.length === 0 && <p className="text-sm text-fg-tertiary">None yet.</p>}
        <ul className="space-y-1">
          {ids.map((s, i) => (
            <li key={s}>
              <button
                type="button"
                className="flex w-full items-center gap-2 rounded-md px-2 py-1 text-left text-sm hover:bg-layer-2"
                onClick={() => void navigate({ to: '/c/$session', params: { session: s } })}
              >
                <span className="min-w-0 flex-1 truncate font-mono text-[13px]">{s}</span>
                {t.checkout?.session === s && <Badge variant="outline">{engineName(mesh, t.checkout.engine)}</Badge>}
                <span className="text-xs text-fg-tertiary tabular-nums">{costs[i]?.data ? costShort(costs[i].data.total) : '—'}</span>
              </button>
            </li>
          ))}
        </ul>
      </Section>
      <Section title="Cost">
        <p className="text-sm tabular-nums">
          {costs.some((c) => c.data) ? costShort(...costs.flatMap((c) => (c.data ? [c.data.total] : []))) : <span className="text-fg-tertiary">— (no session reported cost yet)</span>}
        </p>
      </Section>
    </>
  )
}

/** State-gated actions (67 §7.2, 69): assign, start / back to backlog, send back with notes, mark done, cancel, move project. */
function Actions({ task: t, confirmCancel, setConfirmCancel }: { task: Task; confirmCancel: boolean; setConfirmCancel(v: boolean): void }) {
  const api = useApi()
  const agents = useAgents()
  const invalidate = useInvalidateTasks()
  const navigate = useNavigate()
  const [owner, setOwner] = useState(ownerText(t.owner))
  const [notes, setNotes] = useState('')
  const [project, setProject] = useState<string | null>(t.project)
  const a = actionsFor(t)
  const patch = useMutation({
    mutationFn: (patch: TaskPatch) => api.task_update(t.id, patch),
    onSuccess: () => invalidate(t.id),
    onError: (e) => toast.error(errorText(e)),
  })
  const move = useMoveTask()
  const close = useCloseDone()
  const busy = patch.isPending || move.isPending || close.isPending
  return (
    <Section title="Actions">
      <div className="flex flex-col gap-3 text-sm">
        <div className="flex items-center gap-2">
          <OwnerSelect id="task-owner" value={owner} agents={agents.data ?? []} onChange={setOwner} className="min-w-0 flex-1" />
          <Button size="sm" variant="outline" disabled={busy || owner === ownerText(t.owner)} onClick={() => patch.mutate({ owner: parseOwner(owner) })}>
            {t.owner.type === 'user' && owner === 'user' ? 'Assign' : 'Reassign'}
          </Button>
        </div>
        <p className="text-[13px] text-fg-secondary">
          Reviewer: {ownerLabel(t.reviewer)}
          {t.reviewer === null && " (owner's manager)"}
        </p>
        {a.start && (
          <Button size="sm" disabled={busy} onClick={() => move.mutate({ id: t.id, state: 'todo' })}>
            Start{t.owner.type === 'agent' && ` — queue for ${t.owner.name}'s desk`}
          </Button>
        )}
        {a.toBacklog && (
          <Button size="sm" variant="outline" disabled={busy} onClick={() => move.mutate({ id: t.id, state: 'backlog' })}>
            Back to backlog
          </Button>
        )}
        {a.unblock && (
          <Button size="sm" disabled={busy} onClick={() => move.mutate({ id: t.id, state: 'todo' })}>
            Unblock{t.owner.type === 'agent' && ` — ${wakePhrase(t.owner.name, agents.data, 'wake')}`}
          </Button>
        )}
        {a.sendBack && (
          <div className="flex flex-col gap-1.5">
            <Textarea rows={2} placeholder="Notes for the owner" value={notes} onChange={(e) => setNotes(e.target.value)} />
            <div className="flex gap-2">
              <Button
                size="sm"
                variant="outline"
                className="flex-1"
                disabled={busy}
                onClick={() =>
                  move.mutate(
                    { id: t.id, state: 'doing', note: notes },
                    {
                      onSuccess: () => {
                        setNotes('')
                        if (t.owner.type === 'agent') toast(`woke ${t.owner.name}`)
                      },
                    },
                  )
                }
              >
                Send back
              </Button>
              <Button
                size="sm"
                className="flex-1"
                disabled={busy || !a.canMarkDone}
                title={a.canMarkDone ? undefined : 'Only the reviewer (or the owner when that is you) can mark it done'}
                onClick={() => close.mutate({ id: t.id, because: doneBecause(t) })}
              >
                Mark done
              </Button>
            </div>
          </div>
        )}
        <div className="flex items-center gap-2">
          <ProjectPicker value={project} onChange={setProject} />
          <Button size="sm" variant="outline" disabled={busy || (project ?? null) === t.project} onClick={() => patch.mutate({ project: project?.trim() || null })}>
            Move
          </Button>
        </div>
        {a.cancel && (
          <Button size="sm" variant="ghost" className="self-start text-danger" disabled={busy} onClick={() => setConfirmCancel(true)}>
            Cancel task
          </Button>
        )}
        {t.state === 'done' && <p className="text-[13px] text-fg-tertiary">Done.</p>}
        {t.state === 'cancelled' && <p className="text-[13px] text-fg-tertiary">Cancelled.</p>}
      </div>
      <AlertDialog open={confirmCancel} onOpenChange={setConfirmCancel}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>Cancel this task?</AlertDialogTitle>
            <AlertDialogDescription>{t.title} leaves the board; its thread stays.</AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>Keep</AlertDialogCancel>
            <AlertDialogAction
              onClick={() => move.mutate({ id: t.id, state: 'cancelled' }, { onSuccess: () => void navigate({ to: '/tasks' }) })}
            >
              Cancel task
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </Section>
  )
}
