// 214 D7: the task graph's inline editor — the right edge of the canvas, never a route change. Every field saves on its
// own (`task_update`, serialised per task; a state move through `task_move`; done through the shared close path); text
// saves on blur.
import { useMutation } from '@tanstack/react-query'
import { Link } from '@tanstack/react-router'
import { ExternalLink, X } from 'lucide-react'
import { type ReactNode, useEffect, useState } from 'react'
import { notify as toast } from '@/lib/notify'
import { errorText } from '@/components/settings/shared'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { Skeleton } from '@/components/ui/skeleton'
import { Textarea } from '@/components/ui/textarea'
import { isPopout, openPopout } from '@/lib/popout'
import { useApi } from '@/lib/rpc'
import type { Priority, Task, TaskState } from '@/rpc/bindings/method.roster.output'
import { AttachmentsEditor } from '../Attachments'
import { ProjectCombobox } from '../NewTaskSheet'
import { OwnerSelect } from '../OwnerSelect'
import { AccentField, ChildrenList, ParentBreadcrumb, ParentField } from '../HierarchyFields'
import { TaskChips } from '../TaskChips'
import { ownerText, parseBlocker, parseOwner, shortId, STATES } from '../helpers'
import { useAgents, useCloseDone, useInvalidateTasks, useMoveTask, useTask, useTasks, useTaskUpdate } from '../queries'
import { dependents, PANEL_W, STATE_TONE } from './model'

const PRIORITIES: readonly Priority[] = ['urgent', 'high', 'medium', 'low']

/** Every task `id` waits on, directly or transitively (cycle-safe): none of them may be blocked by `id`. */
function transitiveBlockers(tasks: readonly Pick<Task, 'id' | 'blockers'>[], id: string): Set<string> {
  const byId = new Map(tasks.map((t) => [t.id, t.blockers]))
  const out = new Set<string>()
  const stack = [id]
  while (stack.length) {
    for (const raw of byId.get(stack.pop()!) ?? []) {
      if (out.has(raw) || raw === id) continue
      out.add(raw)
      stack.push(raw)
    }
  }
  return out
}

/**
 * A text field over a record value. Focused, it is the user's draft; on blur a changed draft is saved and shown as
 * `pending` until the save settles (no flash of the old value while the record catches up). When the record changed
 * elsewhere while the user typed, nothing is overwritten: the draft stays in the field with `conflict` set, and the
 * user keeps theirs (`keepMine`) or takes the record's (`takeTheirs`).
 */
function useDraft(value: string, save: (v: string, settled: () => void) => void) {
  const [draft, setDraft] = useState<string | null>(null)
  const [pending, setPending] = useState<string | null>(null)
  // What the field showed when editing began: the record's value and any save still in flight.
  const [base, setBase] = useState<{ value: string; pending: string | null }>({ value, pending: null })
  const [conflict, setConflict] = useState(false)
  useEffect(() => {
    if (pending !== null && value === pending) setPending(null)
  }, [value, pending])
  const commit = (v: string) => {
    setDraft(null)
    setConflict(false)
    setPending(v)
    save(v, () => setPending(null))
  }
  return {
    conflict,
    keepMine: () => draft !== null && commit(draft),
    takeTheirs: () => {
      setDraft(null)
      setConflict(false)
    },
    field: {
      value: draft ?? pending ?? value,
      onChange: (e: { target: { value: string } }) => setDraft(e.target.value),
      onFocus: () => {
        if (draft === null) setBase({ value, pending })
      },
      onBlur: () => {
        if (draft === null || conflict) return
        if (draft === (base.pending ?? base.value)) return setDraft(null)
        if (value !== base.value && value !== base.pending) return setConflict(true)
        commit(draft)
      },
    },
  }
}

/** The note under a field whose record changed while the user typed. */
function Conflict({ draft }: { draft: { conflict: boolean; keepMine(): void; takeTheirs(): void } }) {
  if (!draft.conflict) return null
  return (
    <div role="status" className="flex flex-wrap items-center gap-2 text-xs text-warning">
      Changed elsewhere while you typed — not saved.
      <Button size="xs" variant="outline" onClick={draft.keepMine}>
        Save mine
      </Button>
      <Button size="xs" variant="ghost" onClick={draft.takeTheirs}>
        Use theirs
      </Button>
    </div>
  )
}

export function TaskPanel({ id, known, onSelect, onClose }: { id: string; known: readonly Task[]; onSelect(id: string): void; onClose(): void }) {
  const task = useTask(id)
  // The graph's list already holds the record; `task_get` keeps it fresh when it left the list (moved project, hidden).
  const t = task.data ?? known.find((x) => x.id === id)
  return (
    <aside
      aria-label="Task editor"
      className="absolute inset-y-0 right-0 z-10 flex max-w-full flex-col border-l border-border-subtle bg-background shadow-[-8px_0_24px_-12px_rgb(0_0_0/0.25)]"
      style={{ width: PANEL_W }}
    >
      <header className="flex h-11 shrink-0 items-center gap-2 border-b border-border-subtle pr-1.5 pl-3">
        <span className="font-mono text-xs text-fg-tertiary">#{shortId(id)}</span>
        {/* In a popped graph window the task page opens in a window of its own, so the graph stays put. */}
        {isPopout() ? (
          <Button variant="ghost" size="xs" className="ml-auto text-fg-secondary" onClick={() => void openPopout({ kind: 'page', path: `/tasks/${encodeURIComponent(id)}` })}>
            <ExternalLink /> Open full page
          </Button>
        ) : (
          <Button asChild variant="ghost" size="xs" className="ml-auto text-fg-secondary">
            <Link to="/tasks/$id" params={{ id }}>
              <ExternalLink /> Open full page
            </Link>
          </Button>
        )}
        <Button variant="ghost" size="icon-sm" className="text-fg-secondary" aria-label="Close editor" onClick={onClose}>
          <X />
        </Button>
      </header>
      {!t && task.isLoading && (
        <div className="space-y-3 p-4">
          <Skeleton className="h-8 w-full" />
          <Skeleton className="h-24 w-full" />
        </div>
      )}
      {!t && task.isError && <p className="p-4 text-sm text-fg-tertiary">{task.error.message}</p>}
      {t && <Editor key={t.id} task={t} known={known} onSelect={onSelect} />}
    </aside>
  )
}

function Editor({ task: t, known, onSelect }: { task: Task; known: readonly Task[]; onSelect(id: string): void }) {
  const api = useApi()
  const invalidate = useInvalidateTasks()
  const all = useTasks()
  const allTasks = all.data ?? known
  const agents = useAgents()
  const move = useMoveTask()
  const patch = useTaskUpdate(t.id)
  const patchOther = useMutation({
    mutationFn: ({ id, blockers }: { id: string; blockers: string[] }) => api.task_update(id, { blockers }),
    onSuccess: (updated) => {
      invalidate(t.id)
      invalidate(updated.id)
    },
    onError: (e) => toast.error(errorText(e)),
  })
  const close = useCloseDone()
  const busy = patch.isPending || patchOther.isPending || move.isPending || close.isPending
  const [closing, setClosing] = useState(false)
  const title = useDraft(t.title, (v, settled) => (v.trim() ? patch.mutate({ title: v.trim() }, { onSettled: settled }) : settled()))
  const brief = useDraft(t.brief, (v, settled) => patch.mutate({ brief: v }, { onSettled: settled }))
  const doneWhen = useDraft(t.done_when ?? '', (v, settled) => patch.mutate({ done_when: v.trim() || null }, { onSettled: settled }))
  const byId = new Map(allTasks.map((x) => [x.id, x]))
  const setState = (s: TaskState) => {
    if (s === t.state) return
    if (s === 'done') return setClosing(true)
    setClosing(false)
    move.mutate({ id: t.id, state: s })
  }
  const hasCriteria = !!t.done_when?.trim()
  // Built from the latest saved record at send time (calls queue per task), so quick edits never drop one another.
  const addBlocker = (id: string) => patch.mutate((latest) => ({ blockers: [...new Set([...(latest ?? t).blockers, id])] }))
  const removeBlocker = (raw: string) => patch.mutate((latest) => ({ blockers: (latest ?? t).blockers.filter((x) => x !== raw) }))

  const taskBlockers = t.blockers.filter((raw) => parseBlocker(raw).kind === 'task')
  const blockedByTasks = taskBlockers.flatMap((id) => {
    const found = byId.get(id)
    return found ? [found] : []
  })
  const unresolvedBlockers = t.blockers.filter((raw) => {
    const b = parseBlocker(raw)
    return b.kind !== 'task' || !byId.has(b.id)
  })
  const blocksTasks = t.blocks.flatMap((id) => {
    const found = byId.get(id)
    return found ? [found] : []
  })
  const waitingOnMe = dependents(allTasks, t.id)
  const waitedByMe = transitiveBlockers(allTasks, t.id)

  return (
    <div className="flex min-h-0 flex-1 flex-col gap-4 overflow-y-auto p-4 text-sm">
      <ParentBreadcrumb task={t} onOpen={onSelect} />
      <div className="flex flex-col gap-1.5">
        <Input
          aria-label="Title"
          className="h-9 text-[15px] font-medium"
          {...title.field}
          onKeyDown={(e) => {
            if (e.key === 'Enter') e.currentTarget.blur()
          }}
        />
        <Conflict draft={title} />
      </div>
      <div className="grid grid-cols-2 gap-3">
        <Field label="State" htmlFor="tp-state">
          <Select value={t.state} onValueChange={(v) => setState(v as TaskState)} disabled={busy}>
            <SelectTrigger id="tp-state" size="sm" className="w-full">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              {STATES.map((s) => (
                <SelectItem key={s} value={s}>
                  <span aria-hidden className="size-2 rounded-full" style={{ background: STATE_TONE[s] }} />
                  {s}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </Field>
        <Field label="Priority" htmlFor="tp-priority">
          <Select value={t.priority} onValueChange={(v) => patch.mutate({ priority: v as Priority })} disabled={busy}>
            <SelectTrigger id="tp-priority" size="sm" className="w-full">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              {PRIORITIES.map((p) => (
                <SelectItem key={p} value={p}>
                  {p}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </Field>
      </div>
      {closing && (
        <div role="group" aria-label="Close as done" className="flex flex-col gap-2 rounded-lg border border-success-border bg-success-bg p-3">
          <p className="text-[13px]">Close as done — why?</p>
          <div className="flex flex-wrap gap-2">
            <Button
              size="sm"
              disabled={busy || !hasCriteria}
              title={hasCriteria ? (t.done_when ?? undefined) : 'No done-when criteria stated'}
              onClick={() => close.mutate({ id: t.id, because: { type: 'criteria_met' } }, { onSuccess: () => setClosing(false) })}
            >
              Criteria met
            </Button>
            <Button size="sm" variant="outline" disabled={busy} onClick={() => close.mutate({ id: t.id, because: { type: 'intent_met' } }, { onSuccess: () => setClosing(false) })}>
              Done
            </Button>
            <Button size="sm" variant="ghost" onClick={() => setClosing(false)}>
              Keep open
            </Button>
          </div>
        </div>
      )}
      <Field label="Owner" htmlFor="tp-owner">
        <OwnerSelect id="tp-owner" value={ownerText(t.owner)} agents={agents.data ?? []} onChange={(v) => patch.mutate({ owner: parseOwner(v) })} className="w-full" />
      </Field>
      <Field label="Project" htmlFor="tp-project">
        <ProjectCombobox id="tp-project" value={t.project} onChange={(v) => v !== t.project && patch.mutate({ project: v?.trim() || null })} />
      </Field>
      <Field label="Parent">
        <ParentField task={t} onOpen={onSelect} />
      </Field>
      <Field label="Children">
        <ChildrenList task={t} onOpen={onSelect} />
      </Field>
      <Field label="Accent">
        <AccentField task={t} />
      </Field>
      <Field label="Brief" htmlFor="tp-brief" hint="markdown">
        <Textarea id="tp-brief" rows={6} placeholder="Context, constraints, links" className="text-[13px]" {...brief.field} />
        <Conflict draft={brief} />
      </Field>
      <Field label="Done when" htmlFor="tp-done-when" hint="acceptance criteria">
        <Textarea id="tp-done-when" rows={3} placeholder="What has to be true to close it" className="text-[13px]" {...doneWhen.field} />
        <Conflict draft={doneWhen} />
      </Field>
      <Field label={t.attachments.length ? `Attachments ${t.attachments.length}` : 'Attachments'}>
        <AttachmentsEditor value={t.attachments} onChange={(attachments) => patch.mutate({ attachments })} disabled={busy} />
      </Field>
      <Field label={`Blocked by ${t.blockers.length}`}>
        <TaskChips
          title="Blocked by"
          tasks={blockedByTasks}
          project={t.project}
          exclude={[t.id, ...t.blockers, ...t.blocks, ...waitingOnMe]}
          disabled={busy}
          onAdd={(b) => addBlocker(b.id)}
          onRemove={(b) => removeBlocker(b.id)}
          onOpen={onSelect}
        />
        {unresolvedBlockers.length > 0 && (
          <div className="mt-1.5 flex flex-wrap items-center gap-1.5">
            {unresolvedBlockers.map((raw) => {
              const b = parseBlocker(raw)
              return (
                <Badge key={raw} variant="secondary" className="max-w-full gap-1 pr-1">
                  <span className="truncate font-mono">{b.kind === 'task' ? `task ${shortId(b.id)}` : raw}</span>
                  <button type="button" aria-label={`Remove blocker ${raw}`} className="rounded-full hover:bg-layer-2" disabled={busy} onClick={() => removeBlocker(raw)}>
                    <X className="size-3" />
                  </button>
                </Badge>
              )
            })}
          </div>
        )}
      </Field>
      <Field label={`Blocks ${t.blocks.length}`}>
        <TaskChips
          title="Blocks"
          tasks={blocksTasks}
          project={t.project}
          exclude={[t.id, ...t.blocks, ...taskBlockers, ...waitedByMe]}
          disabled={busy}
          onAdd={(b) => patchOther.mutate({ id: b.id, blockers: [...new Set([...b.blockers, t.id])] })}
          onRemove={(b) => patchOther.mutate({ id: b.id, blockers: b.blockers.filter((x) => x !== t.id) })}
          onOpen={onSelect}
        />
      </Field>
    </div>
  )
}

function Field({ label, htmlFor, hint, children }: { label: string; htmlFor?: string; hint?: string; children: ReactNode }) {
  return (
    <div className="flex flex-col gap-1.5">
      <Label htmlFor={htmlFor} className="text-xs font-semibold tracking-wide text-fg-secondary uppercase">
        {label}
        {hint && <span className="font-normal tracking-normal text-fg-tertiary normal-case"> · {hint}</span>}
      </Label>
      {children}
    </div>
  )
}
