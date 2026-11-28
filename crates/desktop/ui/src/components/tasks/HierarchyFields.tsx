// The hierarchy and accent controls shared by the task page (`TaskDetail`), the modal (`TaskModal`), and the graph's
// inline editor (`TaskPanel`): the parent breadcrumb and picker, the children with their progress and link/create
// actions, and the accent swatches. Every change is one `task_update`; the engine's refusal (a cycle) toasts verbatim.
import { useMutation } from '@tanstack/react-query'
import { Ban, Check, ChevronRight, Link2, Plus, X } from 'lucide-react'
import { useState } from 'react'
import { errorText } from '@/components/settings/shared'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { notify as toast } from '@/lib/notify'
import { useApi } from '@/lib/rpc'
import { cn } from '@/lib/utils'
import type { Task, TaskState } from '@/rpc/bindings/method.roster.output'
import { shortId } from './helpers'
import { ACCENTS, type AccentKey, accentColor, ancestors, childStats, isAccent, parentExcluded, waitingOnChildren } from './hierarchy'
import { TaskPicker } from './TaskPicker'
import { useInvalidateTasks, useTasks, useTaskUpdate } from './queries'
import { openNewTaskModal } from './TaskModal'

const PARENT_STATES: TaskState[] = ['backlog', 'todo', 'doing', 'review', 'blocked', 'done']

/** The accent palette plus "none"; `value` is the current key (`null` = none). */
export function AccentSwatches({ value, onChange, disabled }: { value: string | null; onChange(accent: AccentKey | null): void; disabled?: boolean }) {
  const current = isAccent(value) ? value : null
  return (
    <div role="radiogroup" aria-label="Accent colour" className="flex flex-wrap items-center gap-1">
      <button
        type="button"
        role="radio"
        aria-checked={current === null}
        aria-label="No accent"
        title="No accent"
        disabled={disabled}
        onClick={() => onChange(null)}
        className={cn('grid size-5 place-items-center rounded-full border border-border-subtle text-fg-tertiary', current === null && 'ring-2 ring-primary ring-offset-1 ring-offset-background')}
      >
        <Ban className="size-3" />
      </button>
      {ACCENTS.map((k) => (
        <button
          key={k}
          type="button"
          role="radio"
          aria-checked={current === k}
          aria-label={k}
          title={k}
          disabled={disabled}
          onClick={() => onChange(k)}
          className={cn('grid size-5 place-items-center rounded-full', current === k && 'ring-2 ring-primary ring-offset-1 ring-offset-background')}
          style={{ background: accentColor(k) }}
        >
          {current === k && <Check className="size-3 text-background" />}
        </button>
      ))}
    </div>
  )
}

/** Every task (any project): the parent may sit in another project, and ancestors and children are looked up here. */
function useAllTasks(): readonly Task[] {
  return useTasks().data ?? []
}

/** Root-first ancestors of `task`, each opening its task; nothing for a top-level task. */
export function ParentBreadcrumb({ task, onOpen }: { task: Task; onOpen(id: string): void }) {
  const all = useAllTasks()
  const chain = ancestors(all, task.id)
  // A parent the list does not hold (gone, or not replicated yet) still shows by id.
  if (chain.length === 0 && !task.parent) return null
  return (
    <nav aria-label="Parent tasks" className="flex min-w-0 flex-wrap items-center gap-0.5 text-xs text-fg-secondary">
      {chain.length === 0 && task.parent && <span className="font-mono">task {shortId(task.parent)}</span>}
      {chain.map((a) => (
        <span key={a.id} className="flex min-w-0 items-center gap-0.5">
          <button type="button" className="max-w-48 truncate hover:text-foreground hover:underline" onClick={() => onOpen(a.id)}>
            {a.title}
          </button>
          <ChevronRight className="size-3 shrink-0 text-fg-tertiary" aria-hidden />
        </span>
      ))}
    </nav>
  )
}

/** The parent: its title (opens it), clear, and a picker that hides the task and everything under it. */
export function ParentField({ task, onOpen }: { task: Task; onOpen(id: string): void }) {
  const all = useAllTasks()
  const patch = useTaskUpdate(task.id)
  const [picking, setPicking] = useState(false)
  const parent = task.parent ? all.find((t) => t.id === task.parent) : undefined
  return (
    <div className="flex flex-wrap items-center gap-1.5">
      {task.parent && (
        <Badge variant="secondary" className="max-w-full gap-1 pr-1">
          <button type="button" className="truncate hover:underline" onClick={() => onOpen(task.parent!)}>
            {parent?.title ?? `task ${shortId(task.parent)}`}
          </button>
          <button type="button" aria-label="Clear parent" className="rounded-full hover:bg-layer-2" disabled={patch.isPending} onClick={() => patch.mutate({ parent: '' })}>
            <X className="size-3" />
          </button>
        </Badge>
      )}
      <Button type="button" variant="outline" size="xs" disabled={patch.isPending} onClick={() => setPicking(true)}>
        {task.parent ? 'Change parent…' : 'Set parent…'}
      </Button>
      {picking && (
        <TaskPicker
          open
          onOpenChange={setPicking}
          title="Parent task"
          project={null}
          states={PARENT_STATES}
          // A task under this one would make it its own ancestor (the engine refuses it too).
          exclude={[...parentExcluded(all, task.id), ...(task.parent ? [task.parent] : [])]}
          onPick={(p) => patch.mutate({ parent: p.id })}
        />
      )}
    </div>
  )
}

/** The direct children, each opening its task, with "n done of m", derived wait, unlink, link-existing, and new subtask. */
export function ChildrenList({
  task,
  onOpen,
  onNewSubtask,
}: {
  task: Task
  onOpen(id: string): void
  onNewSubtask?: () => void
}) {
  const api = useApi()
  const invalidate = useInvalidateTasks()
  const all = useAllTasks()
  const [picking, setPicking] = useState(false)
  const children = all.filter((t) => t.parent === task.id)
  const stats = childStats(children).get(task.id)
  const wait = waitingOnChildren(stats)

  const reparent = useMutation({
    mutationFn: ({ childId, parent }: { childId: string; parent: string }) => api.task_update(childId, { parent }),
    onSuccess: () => invalidate(task.id),
    onError: (e) => toast.error(errorText(e)),
  })

  const excludeIds = [
    task.id,
    ...children.map((c) => c.id),
    ...ancestors(all, task.id).map((a) => a.id),
  ]

  return (
    <div className="flex flex-col gap-2">
      {stats && stats.total > 0 && (
        <div className="flex flex-wrap items-center gap-2 text-xs text-fg-secondary">
          <div className="h-1.5 w-24 overflow-hidden rounded-full bg-layer-2" aria-hidden>
            <div className="h-full bg-success" style={{ width: `${(stats.done / stats.total) * 100}%` }} />
          </div>
          <span className="tabular-nums">
            {stats.done} done of {stats.total}
          </span>
          {wait && (
            <Badge variant="outline" className="h-5 px-1.5 text-[11px] font-normal text-fg-secondary">
              {wait}
            </Badge>
          )}
        </div>
      )}
      {children.length === 0 ? (
        <p className="text-xs text-fg-tertiary">No subtasks yet.</p>
      ) : (
        <ul className="flex flex-col gap-1 text-sm">
          {children.map((c) => (
            <li key={c.id} className="group flex min-w-0 items-center gap-2 rounded-md px-1 py-0.5 hover:bg-layer-1">
              <span
                className={cn(
                  'size-1.5 shrink-0 rounded-full',
                  c.state === 'done' ? 'bg-success' : c.state === 'cancelled' ? 'bg-fg-tertiary' : 'bg-info',
                )}
                aria-hidden
              />
              <button
                type="button"
                className={cn('min-w-0 flex-1 truncate text-left hover:underline', c.state === 'cancelled' && 'line-through')}
                onClick={() => onOpen(c.id)}
              >
                {c.title}
              </button>
              <Badge variant="outline" className="h-5 shrink-0 px-1.5 text-[11px] font-normal text-fg-secondary">
                {c.state}
              </Badge>
              <button
                type="button"
                aria-label={`Unlink ${c.title}`}
                title="Unlink child"
                disabled={reparent.isPending}
                className="flex size-5 shrink-0 items-center justify-center rounded-full text-fg-tertiary hover:bg-layer-2 hover:text-foreground disabled:opacity-50"
                onClick={() => reparent.mutate({ childId: c.id, parent: '' })}
              >
                <X className="size-3" />
              </button>
            </li>
          ))}
        </ul>
      )}
      <div className="flex flex-wrap items-center gap-1.5">
        <Button type="button" variant="outline" size="xs" disabled={reparent.isPending} onClick={() => setPicking(true)}>
          <Link2 className="size-3" /> Link existing child…
        </Button>
        <Button
          type="button"
          variant="outline"
          size="xs"
          onClick={() => (onNewSubtask ? onNewSubtask() : openNewTaskModal({ parent: task.id, project: task.project }))}
        >
          <Plus className="size-3" /> New subtask
        </Button>
      </div>
      {picking && (
        <TaskPicker
          open
          onOpenChange={setPicking}
          title="Link existing child"
          project={null}
          states={PARENT_STATES}
          exclude={excludeIds}
          onPick={(picked) => reparent.mutate({ childId: picked.id, parent: task.id })}
        />
      )}
    </div>
  )
}

/** The accent swatches of one task, saved on click. */
export function AccentField({ task }: { task: Task }) {
  const patch = useTaskUpdate(task.id)
  return <AccentSwatches value={task.accent} disabled={patch.isPending} onChange={(a) => patch.mutate({ accent: a ?? 'none' })} />
}
