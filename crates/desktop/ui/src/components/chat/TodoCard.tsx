import { ChevronRight } from 'lucide-react'
import { useEffect } from 'react'
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from '@/components/ui/collapsible'
import { usePanes } from '@/lib/panes'
import { cn } from '@/lib/utils'
import type { Todo } from '@/rpc/bindings/Todo'
import type { TodoSnapshot } from '@/rpc/bindings/TodoSnapshot'
import { useScope } from '@/store/engine-scope'
import { useSessionField } from '@/store/sessions'
import { surfaceOf } from './slash'

const LABEL: Record<Todo['status'], string> = {
  pending: 'pending',
  in_progress: 'in progress',
  done: 'done',
  dropped: 'dropped',
  blocked: 'blocked',
}

/** The compact row (47 §6.1): `2/5 done · 1 dropped · 1 blocked` and the task in progress, else the first pending, else `Waiting: N blocked`. */
export function summarize(items: Todo[]): { counts: string; now: string | null } {
  let done = 0
  let dropped = 0
  let blocked = 0
  let current: Todo | undefined
  let pending: Todo | undefined
  for (const t of items) {
    if (t.status === 'done') done++
    else if (t.status === 'dropped') dropped++
    else if (t.status === 'blocked') blocked++
    else if (t.status === 'in_progress') current ??= t
    else pending ??= t
  }
  const counts = [`${done}/${items.length} done`]
  if (dropped) counts.push(`${dropped} dropped`)
  if (blocked) counts.push(`${blocked} blocked`)
  const next = current ?? pending
  return { counts: counts.join(' · '), now: next ? `Now: ${next.text}` : blocked ? `Waiting: ${blocked} blocked` : null }
}

/**
 * What the card says when there is no list to disclose (47 §6.2): `null` means an authoritative empty plan,
 * the one state that hides. Unknown (`null` snapshot) and a possibly outdated empty list never read as "nothing to do".
 */
export function note(todos: TodoSnapshot | null, stale: boolean): string | null {
  if (!todos) return stale ? 'unavailable' : 'loading…'
  return todos.items.length || !stale ? null : 'empty · Stale'
}

/**
 * The session's persistent plan (47 §6.2): chrome between transcript and composer, never a transcript row.
 * Collapsed by default; the user's choice lives in the session slice, so it survives snapshots and session
 * switches. Hidden only for an authoritative empty plan; a stale list stays visible and says so.
 */
export function TodoCard({ session }: { session: string }) {
  const { sessions: store } = useScope()
  const todos = useSessionField(session, 'todos') ?? null
  const stale = useSessionField(session, 'todosStale') ?? false
  const expanded = useSessionField(session, 'todosExpanded') ?? false

  // `/todos` toggles the disclosure without moving focus (the composer keeps it); only in the pane that typed it.
  const focused = usePanes((s) => s.focused === session)
  useEffect(() => {
    if (!focused) return
    const on = (e: Event) => {
      if (surfaceOf(e) !== 'todos') return
      const { sessions, setTodosExpanded } = store.getState()
      setTodosExpanded(session, !sessions[session]?.todosExpanded)
    }
    window.addEventListener('theuth:open', on)
    return () => window.removeEventListener('theuth:open', on)
  }, [session, store, focused])

  if (!todos?.items.length) {
    const text = note(todos, stale)
    if (!text) return null
    return (
      <div className="mx-auto w-full max-w-[768px] px-4 pb-2">
        <p role="status" className="flex min-h-11 items-center gap-2 rounded-xl border bg-background px-3 py-2 text-sm">
          <span className="font-medium">Todos</span>
          <span className="text-fg-tertiary">· {text}</span>
        </p>
      </div>
    )
  }
  const { counts, now } = summarize(todos.items)
  const tags = [todos.source === 'acp' && 'External plan', stale && 'Stale'].filter(Boolean).join(' · ')

  return (
    <Collapsible
      open={expanded}
      onOpenChange={(open) => store.getState().setTodosExpanded(session, open)}
      className="mx-auto w-full max-w-[768px] px-4 pb-2"
    >
      <div className="rounded-xl border bg-background text-sm">
        <CollapsibleTrigger className="flex min-h-11 w-full items-center gap-2 rounded-xl px-3 py-2 text-left outline-none hover:bg-layer-2 focus-visible:ring-[3px] focus-visible:ring-ring/50">
          <ChevronRight className={cn('size-4 shrink-0 text-fg-tertiary transition-transform', expanded && 'rotate-90')} />
          <span className="shrink-0 font-medium">Todos</span>
          <span className="shrink-0 text-fg-secondary tabular-nums">· {counts}</span>
          {tags && <span className="shrink-0 text-xs text-fg-tertiary">· {tags}</span>}
          {!expanded && now && <span className="min-w-0 flex-1 truncate text-fg-secondary">{now}</span>}
        </CollapsibleTrigger>
        {/* Polite progress: counts and the current task, not the whole list. */}
        <p role="status" className="sr-only">
          Todos: {counts}.{now && ` ${now}.`}
        </p>
        <CollapsibleContent>
          {/* Bounded by the visible band (a third of it; the keyboard shrinks it further) and scrolls inside, never behind the composer. */}
          <ul className="max-h-[calc(var(--visible-band,100dvh)/3)] overflow-y-auto border-t px-3 py-2 wrap-anywhere">
            {/* Rows are not interactive; position is identity enough. */}
            {todos.items.map((t, i) => (
              <li key={i} className={cn('flex gap-3 py-1', (t.status === 'done' || t.status === 'dropped') && 'text-fg-tertiary')}>
                <span className="w-20 shrink-0 pt-px text-xs text-fg-tertiary">[{LABEL[t.status]}]</span>
                <span className="min-w-0 flex-1">
                  {t.text}
                  {t.reason && <span className="block text-fg-secondary">{t.reason}</span>}
                </span>
              </li>
            ))}
          </ul>
        </CollapsibleContent>
      </div>
    </Collapsible>
  )
}
