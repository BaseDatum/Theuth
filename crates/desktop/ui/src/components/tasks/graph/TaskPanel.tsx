// 214 D7: the task graph's inline editor — the right edge of the canvas, never a route change. Every field saves on its
// own (`task_update`; a state move through `task_move`, closing as done with a `done_because`); text saves on blur.
import { useMutation } from '@tanstack/react-query'
import { Link } from '@tanstack/react-router'
import { ExternalLink, X } from 'lucide-react'
import { type ReactNode, useEffect, useState } from 'react'
import { toast } from 'sonner'
import { errorText } from '@/components/settings/shared'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { Skeleton } from '@/components/ui/skeleton'
import { Textarea } from '@/components/ui/textarea'
import { useApi } from '@/lib/rpc'
import type { DoneBecause, TaskPatch } from '@/rpc/bindings/method.roster.input'
import type { Priority, Task, TaskState } from '@/rpc/bindings/method.roster.output'
import { ProjectCombobox } from '../NewTaskSheet'
import { OwnerSelect } from '../OwnerSelect'
import { TaskPicker } from '../TaskPicker'
import { ownerText, parseBlocker, parseOwner, shortId, STATES } from '../helpers'
import { useAgents, useInvalidateTasks, useMoveTask, useTask } from '../queries'
import { PANEL_W, STATE_TONE } from './model'

const PRIORITIES: readonly Priority[] = ['urgent', 'high', 'medium', 'low']

/** A text field's local draft: follows the record until focused, saves a changed value on blur. */
function useDraft(value: string, save: (v: string) => void) {
  const [draft, setDraft] = useState(value)
  const [editing, setEditing] = useState(false)
  useEffect(() => {
    if (!editing) setDraft(value)
  }, [value, editing])
  return {
    value: draft,
    onChange: (e: { target: { value: string } }) => setDraft(e.target.value),
    onFocus: () => setEditing(true),
    onBlur: () => {
      setEditing(false)
      if (draft !== value) save(draft)
    },
  }
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
        <Button asChild variant="ghost" size="xs" className="ml-auto text-fg-secondary">
          <Link to="/tasks/$id" params={{ id }}>
            <ExternalLink /> Open full page
          </Link>
        </Button>
        <Button variant="ghost" size="icon-sm" className="text-fg-secondary" aria-label="Close editor" onClick={onClose}>
          <X />
        </Button>
      </header>
      {!t && task.isPending && (
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
  const agents = useAgents()
  const invalidate = useInvalidateTasks()
  const move = useMoveTask()
  const patch = useMutation({
    mutationFn: (p: TaskPatch) => api.task_update(t.id, p),
    onSuccess: () => invalidate(t.id),
    onError: (e) => toast.error(errorText(e)),
  })
  const busy = patch.isPending || move.isPending
  const [closing, setClosing] = useState(false)
  const [picking, setPicking] = useState(false)
  const title = useDraft(t.title, (v) => (v.trim() ? patch.mutate({ title: v.trim() }) : undefined))
  const brief = useDraft(t.brief, (v) => patch.mutate({ brief: v }))
  const doneWhen = useDraft(t.done_when ?? '', (v) => patch.mutate({ done_when: v.trim() || null }))
  const byId = new Map(known.map((x) => [x.id, x]))
  const setState = (s: TaskState) => {
    if (s === t.state) return
    if (s === 'done') return setClosing(true)
    setClosing(false)
    move.mutate({ id: t.id, state: s })
  }
  const closeDone = (because: DoneBecause) => patch.mutate({ state: 'done', done_because: because }, { onSuccess: () => setClosing(false) })
  const setBlockers = (blockers: string[]) => patch.mutate({ blockers })
  const blocks = t.blocks.map((b) => byId.get(b)).filter((x): x is Task => x !== undefined)

  return (
    <div className="flex min-h-0 flex-1 flex-col gap-4 overflow-y-auto p-4 text-sm">
      <Input
        aria-label="Title"
        className="h-9 text-[15px] font-medium"
        {...title}
        onKeyDown={(e) => {
          if (e.key === 'Enter') e.currentTarget.blur()
        }}
      />
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
            <Button size="sm" disabled={busy} onClick={() => closeDone({ type: 'criteria_met' })} title={t.done_when ?? undefined}>
              Criteria met
            </Button>
            <Button size="sm" variant="outline" disabled={busy} onClick={() => closeDone({ type: 'intent_met' })}>
              Intent met
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
      <Field label="Brief" htmlFor="tp-brief" hint="markdown">
        <Textarea id="tp-brief" rows={6} placeholder="Context, constraints, links" className="text-[13px]" {...brief} />
      </Field>
      <Field label="Done when" htmlFor="tp-done-when" hint="acceptance criteria">
        <Textarea id="tp-done-when" rows={3} placeholder="What has to be true to close it" className="text-[13px]" {...doneWhen} />
      </Field>
      <Field label={`Blocked by ${t.blockers.length}`}>
        <div className="flex flex-wrap items-center gap-1.5">
          {t.blockers.map((raw) => {
            const b = parseBlocker(raw)
            const other = b.kind === 'task' ? byId.get(b.id) : undefined
            return (
              <Badge key={raw} variant="secondary" className="max-w-full gap-1 pr-1">
                {other ? (
                  <button type="button" className="truncate hover:underline" onClick={() => onSelect(other.id)}>
                    {other.title}
                  </button>
                ) : (
                  <span className="truncate font-mono">{b.kind === 'task' ? `task ${shortId(b.id)}` : raw}</span>
                )}
                <button
                  type="button"
                  aria-label={`Remove blocker ${other?.title ?? raw}`}
                  className="rounded-full hover:bg-layer-2"
                  disabled={busy}
                  onClick={() => setBlockers(t.blockers.filter((x) => x !== raw))}
                >
                  <X className="size-3" />
                </button>
              </Badge>
            )
          })}
          <Button type="button" variant="outline" size="xs" disabled={busy} onClick={() => setPicking(true)}>
            Add blocker…
          </Button>
        </div>
        <TaskPicker
          open={picking}
          onOpenChange={setPicking}
          title="Blocked by"
          project={t.project}
          exclude={[t.id, ...t.blockers, ...t.blocks]}
          onPick={(b) => setBlockers([...t.blockers, b.id])}
        />
      </Field>
      {blocks.length > 0 && (
        <Field label={`Blocks ${t.blocks.length}`}>
          <div className="flex flex-wrap gap-1.5">
            {blocks.map((b) => (
              <Badge key={b.id} variant="outline" className="max-w-full">
                <button type="button" className="truncate hover:underline" onClick={() => onSelect(b.id)}>
                  {b.title}
                </button>
              </Badge>
            ))}
          </div>
        </Field>
      )}
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
