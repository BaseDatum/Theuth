// "+ Add → Tasks…": pick tasks (search, tick several) or every task of a project; the shell places them as task cards
// in a new frame, laid out by blockers, with the blocker links (`task-cards.ts` `addTasks`).

import { Check } from 'lucide-react'
import { useMemo, useState } from 'react'
import { ownerLabel, shortId } from '@/components/tasks/helpers'
import { EMPTY_FILTER, useBoardProjects, useTasks } from '@/components/tasks/queries'
import type { Task } from '@/rpc/bindings/method.roster.output'

const row = 'wb-press flex min-h-10 w-full items-center gap-2 rounded-xl px-3 text-left text-sm'

/** `title`: the frame's — the project's name when every pick is of one project, else "Tasks". */
export function framedTitle(tasks: readonly Pick<Task, 'project'>[]): string {
  const projects = new Set(tasks.map((t) => t.project))
  const [only] = projects
  return projects.size === 1 && only ? only : 'Tasks'
}

export function TasksPicker({ onPick, onDone }: { onPick(tasks: Task[], title: string): void; onDone(): void }) {
  const projects = useBoardProjects()
  const [project, setProject] = useState<string | null>(null)
  const [query, setQuery] = useState('')
  const [picked, setPicked] = useState<ReadonlySet<string>>(new Set())
  const list = useTasks({ ...EMPTY_FILTER, project })
  const shown = useMemo(() => {
    const q = query.trim().toLowerCase()
    return (list.data ?? []).filter((t) => !q || t.title.toLowerCase().includes(q) || t.id.toLowerCase().endsWith(q))
  }, [list.data, query])
  const pick = (tasks: Task[], title = framedTitle(tasks)) => {
    if (!tasks.length) return
    onPick(tasks, title)
    onDone()
  }
  const toggle = (id: string) =>
    setPicked((s) => {
      const n = new Set(s)
      if (!n.delete(id)) n.add(id)
      return n
    })
  return (
    <div className="absolute inset-0 z-50 grid place-items-center bg-black/20" onPointerDown={(e) => e.target === e.currentTarget && onDone()}>
      <div
        role="dialog"
        aria-label="Add tasks"
        className="wb-glass flex max-h-[75%] w-[min(420px,calc(100vw-32px))] flex-col gap-2 rounded-2xl p-3"
        onKeyDown={(e) => {
          e.stopPropagation()
          if (e.key === 'Escape') onDone()
        }}
      >
        <h2 className="px-1 text-sm font-semibold">Add tasks</h2>
        <div className="flex gap-2">
          <select
            aria-label="Project"
            className="min-h-9 min-w-0 flex-1 rounded-lg border border-border-subtle bg-transparent px-2 text-sm"
            value={project ?? ''}
            onChange={(e) => {
              setProject(e.target.value || null)
              setPicked(new Set())
            }}
          >
            <option value="">All projects</option>
            {projects.map((p) => (
              <option key={p} value={p}>
                {p}
              </option>
            ))}
          </select>
          {project && (
            <button type="button" className="wb-press rounded-lg px-2.5 text-sm font-medium" disabled={!list.data?.length} onClick={() => pick(list.data ?? [], project)}>
              All of {project}
            </button>
          )}
        </div>
        <input
          autoFocus
          aria-label="Search tasks"
          placeholder="Search tasks by title…"
          className="min-h-9 rounded-lg border border-border-subtle bg-transparent px-2.5 text-sm"
          value={query}
          onChange={(e) => setQuery(e.target.value)}
        />
        <ul className="flex min-h-0 flex-col overflow-y-auto" aria-label="Tasks" aria-multiselectable>
          {shown.map((t) => {
            const on = picked.has(t.id)
            return (
              <li key={t.id}>
                <button type="button" role="option" aria-selected={on} className={row} onClick={() => toggle(t.id)}>
                  <span className={`grid size-4 shrink-0 place-items-center rounded border ${on ? 'border-fg-accent bg-fg-accent text-background' : 'border-border-strong'}`} aria-hidden>
                    {on && <Check className="size-3" />}
                  </span>
                  <span className="min-w-0 flex-1 truncate">{t.title}</span>
                  <span className="shrink-0 text-xs text-fg-tertiary">
                    {t.state} · {ownerLabel(t.owner)} · {shortId(t.id)}
                  </span>
                </button>
              </li>
            )
          })}
          {list.data && shown.length === 0 && <li className="px-3 py-2 text-sm text-fg-tertiary">No matching task.</li>}
          {list.isPending && <li className="px-3 py-2 text-sm text-fg-tertiary">Loading…</li>}
          {list.isError && <li className="px-3 py-2 text-sm text-fg-tertiary">{list.error.message}</li>}
        </ul>
        <div className="flex justify-end gap-2">
          <button type="button" className="wb-press rounded-lg px-3 py-1.5 text-sm" onClick={onDone}>
            Cancel
          </button>
          <button
            type="button"
            className="wb-press rounded-lg px-3 py-1.5 text-sm font-semibold disabled:opacity-40"
            disabled={picked.size === 0}
            onClick={() => pick((list.data ?? []).filter((t) => picked.has(t.id)))}
          >
            Add {picked.size || ''} {picked.size === 1 ? 'task' : 'tasks'}
          </button>
        </div>
      </div>
    </div>
  )
}
