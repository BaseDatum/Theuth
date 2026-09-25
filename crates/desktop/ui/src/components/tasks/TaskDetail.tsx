import { useMutation, useQueries } from '@tanstack/react-query'
import { Link, useNavigate } from '@tanstack/react-router'
import { ExternalLink, MessageSquare } from 'lucide-react'
import { type ReactNode, useMemo, useState } from 'react'
import { toast } from 'sonner'
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
import type { Task } from '@/rpc/bindings/Task'
import type { TaskEvent } from '@/rpc/bindings/TaskEvent'
import type { TaskPatch } from '@/rpc/bindings/TaskPatch'
import { OwnerSelect } from './OwnerSelect'
import { actionsFor, age, ownerLabel, ownerText, parseBlocker, parseOwner, shortId } from './helpers'
import { useAgents, useInvalidateTasks, useTask, useTaskEvents } from './queries'

/** `blocks` is not in the contract yet; the engine may add it to `task_get` — render it when present. */
type TaskWithBlocks = Task & { blocks?: string[] }

/**
 * `/tasks/$id` (67 §12): brief, blockers and blocks, placement, the thread document with a comment box, sessions,
 * cost, the field-level event log, and the actions the state allows. Every action is one RPC; errors toast verbatim.
 */
export function TaskDetail({ id }: { id: string }) {
  const task = useTask(id)
  const events = useTaskEvents(id)
  if (task.isPending) {
    return (
      <Frame backTo="/tasks" backLabel="Back to tasks" title={<Skeleton className="h-5 w-48" />}>
        <div className="space-y-3 p-6">
          <Skeleton className="h-4 w-2/3" />
          <Skeleton className="h-4 w-1/2" />
        </div>
      </Frame>
    )
  }
  if (task.isError) {
    return (
      <Frame backTo="/tasks" backLabel="Back to tasks" title={<h2 className="text-sm font-semibold">Task</h2>}>
        <p className="p-6 text-center text-sm text-fg-tertiary">{task.error.message}</p>
      </Frame>
    )
  }
  const t = task.data as TaskWithBlocks
  return (
    <Frame
      backTo="/tasks"
      backLabel="Back to tasks"
      title={
        <h2 className="flex min-w-0 flex-1 items-center gap-2 truncate text-sm font-semibold">
          <ProjectIcon project={t.project} className="size-4" />
          <span className="truncate">{t.title}</span>
          <Badge variant="outline">{t.state}</Badge>
        </h2>
      }
    >
      <div className="min-h-0 flex-1 overflow-y-auto">
        <div className="mx-auto flex w-full max-w-[1100px] flex-col gap-6 p-4 md:flex-row">
          <div className="flex min-w-0 flex-1 flex-col gap-6">
            <Section title="Brief">
              <Streamdown mode="static" className="text-sm">
                {t.brief || '_No brief._'}
              </Streamdown>
            </Section>
            {(t.blockers.length > 0 || (t.blocks?.length ?? 0) > 0) && (
              <div className="grid gap-6 sm:grid-cols-2">
                <Section title={`Waiting on ${t.blockers.length}`}>
                  <ul className="space-y-1 text-sm">
                    {t.blockers.map((raw) => {
                      const b = parseBlocker(raw)
                      return (
                        <li key={raw}>
                          {b.kind === 'task' ? (
                            <Link to="/tasks/$id" params={{ id: b.id }} className="text-primary hover:underline">
                              task {shortId(b.id)}
                            </Link>
                          ) : (
                            <span className="font-mono text-fg-secondary">{raw}</span>
                          )}
                        </li>
                      )
                    })}
                  </ul>
                </Section>
                {t.blocks && t.blocks.length > 0 && (
                  <Section title={`Blocks ${t.blocks.length}`}>
                    <ul className="space-y-1 text-sm">
                      {t.blocks.map((bid) => (
                        <li key={bid}>
                          <Link to="/tasks/$id" params={{ id: bid }} className="text-primary hover:underline">
                            task {shortId(bid)}
                          </Link>
                        </li>
                      ))}
                    </ul>
                  </Section>
                )}
              </div>
            )}
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
                    {e.field === 'comment' ? (
                      <span className="wrap-anywhere">{e.after}</span>
                    ) : (
                      <span className="wrap-anywhere text-fg-secondary">
                        {e.before ?? '—'} → {e.after ?? '—'}
                      </span>
                    )}
                  </li>
                ))}
              </ol>
            </Section>
          </div>
          <aside className="flex w-full shrink-0 flex-col gap-6 md:w-72">
            <Actions task={t} />
            <Sessions task={t} events={events.data ?? []} />
            {t.external.length > 0 && (
              <Section title="External">
                <ul className="space-y-1 text-sm">
                  {t.external.map((u) => (
                    <li key={u}>
                      <a href={u} target="_blank" rel="noreferrer" className="flex items-center gap-1 text-primary hover:underline">
                        <ExternalLink className="size-3.5" />
                        <span className="truncate">{u}</span>
                      </a>
                    </li>
                  ))}
                </ul>
              </Section>
            )}
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
  const reason = [...events].reverse().find((e) => e.field === 'placement' || e.field === 'where')
  return (
    <Section title="Placement">
      <div className="flex flex-wrap items-center gap-1.5 text-sm">
        {t.where ? <Badge variant="outline">{t.where}</Badge> : <span className="text-fg-tertiary">not placed yet</span>}
        {t.harness && <Badge variant="outline">{t.harness}</Badge>}
        {t.checkout && (
          <span className="text-fg-secondary">
            · checked out on {t.checkout.engine} (epoch {t.checkout.epoch}), progress {age(t.checkout.progress_at_ms)} ago
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

/** The 59 document behind the task, read-only when the harness says so, plus a comment box → `task_comment`. */
function Thread({ task: t }: { task: Task }) {
  const api = useApi()
  const invalidate = useInvalidateTasks()
  const [text, setText] = useState('')
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
        <Textarea rows={2} placeholder="Comment — wakes the owner" value={text} onChange={(e) => setText(e.target.value)} />
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
  const api = useApi()
  const navigate = useNavigate()
  const ids = [...new Set([t.checkout?.session, ...events.map((e) => e.session)].filter((s): s is string => !!s))]
  const costs = useQueries({
    queries: ids.map((s) => ({ queryKey: queryKeys.sessionCost(s), queryFn: () => api.session_cost(s), retry: false })),
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
                {t.checkout?.session === s && <Badge variant="outline">{t.checkout.engine}</Badge>}
                <span className="text-xs text-fg-tertiary tabular-nums">{costs[i]?.data ? costShort(costs[i].data.total) : '—'}</span>
              </button>
            </li>
          ))}
        </ul>
      </Section>
      <Section title="Cost">
        <p className="text-sm tabular-nums">
          {costs.some((c) => c.data) ? costs.flatMap((c) => (c.data ? [costShort(c.data.total)] : [])).join(' + ') : <span className="text-fg-tertiary">— (no session reported cost yet)</span>}
        </p>
      </Section>
    </>
  )
}

/** State-gated actions (67 §7.2): assign, send back with notes, mark done, cancel, move project. */
function Actions({ task: t }: { task: Task }) {
  const api = useApi()
  const agents = useAgents()
  const invalidate = useInvalidateTasks()
  const navigate = useNavigate()
  const [owner, setOwner] = useState(ownerText(t.owner))
  const [notes, setNotes] = useState('')
  const [project, setProject] = useState<string | null>(t.project)
  const [confirmCancel, setConfirmCancel] = useState(false)
  const a = actionsFor(t)
  const patch = useMutation({
    mutationFn: async ({ patch, note }: { patch: TaskPatch; note?: string }) => {
      const r = await api.task_update(t.id, patch)
      if (note) await api.task_comment(t.id, note)
      return r
    },
    onSuccess: () => {
      setNotes('')
      invalidate(t.id)
    },
    onError: (e) => toast.error(errorText(e)),
  })
  const busy = patch.isPending
  return (
    <Section title="Actions">
      <div className="flex flex-col gap-3 text-sm">
        <div className="flex items-center gap-2">
          <OwnerSelect value={owner} agents={agents.data ?? []} onChange={setOwner} className="min-w-0 flex-1" />
          <Button size="sm" variant="outline" disabled={busy || owner === ownerText(t.owner)} onClick={() => patch.mutate({ patch: { owner: parseOwner(owner) } })}>
            {t.owner.type === 'user' && owner === 'user' ? 'Assign' : 'Reassign'}
          </Button>
        </div>
        <p className="text-[13px] text-fg-secondary">
          Reviewer: {ownerLabel(t.reviewer)}
          {t.reviewer === null && " (owner's manager)"}
        </p>
        {a.sendBack && (
          <div className="flex flex-col gap-1.5">
            <Textarea rows={2} placeholder="Notes for the owner" value={notes} onChange={(e) => setNotes(e.target.value)} />
            <div className="flex gap-2">
              <Button size="sm" variant="outline" className="flex-1" disabled={busy} onClick={() => patch.mutate({ patch: { state: 'doing' }, note: notes.trim() || undefined })}>
                Send back
              </Button>
              <Button
                size="sm"
                className="flex-1"
                disabled={busy || !a.canMarkDone}
                title={a.canMarkDone ? undefined : 'Only the reviewer (or the owner when that is you) can mark it done'}
                onClick={() => patch.mutate({ patch: { state: 'done' } })}
              >
                Mark done
              </Button>
            </div>
          </div>
        )}
        <div className="flex items-center gap-2">
          <ProjectPicker value={project} onChange={setProject} />
          <Button size="sm" variant="outline" disabled={busy || (project ?? null) === t.project} onClick={() => patch.mutate({ patch: { project: project?.trim() || null } })}>
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
              onClick={() =>
                patch.mutate(
                  { patch: { state: 'cancelled' } },
                  { onSuccess: () => void navigate({ to: '/tasks' }) },
                )
              }
            >
              Cancel task
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </Section>
  )
}
