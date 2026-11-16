import { cn } from '@/lib/utils'
import { backgroundColors } from '@/lib/background-colors'
import type { Todo } from "@/rpc/bindings/method.agent.output"
import type { TodoSnapshot } from "@/rpc/bindings/method.agent.output"
import { useScope } from '@/store/engine-scope'
import { useSessionField } from '@/store/sessions'
import { BackgroundCard } from './BackgroundCard'

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

/** The plan card; the shared background area owns visibility and the grabber. */
export function TodoCard({ session }: { session: string }) {
  const { sessions: store } = useScope()
  const todos = useSessionField(session, 'todos') ?? null
  const stale = useSessionField(session, 'todosStale') ?? false
  const expanded = useSessionField(session, 'todosExpanded') ?? false
  const items = todos?.items ?? []
  const text = items.length ? null : note(todos, stale)
  if (!items.length && !text) return null
  const { counts, now } = summarize(items)
  const tags = [todos?.source === 'acp' && 'External plan', stale && 'Stale'].filter(Boolean).join(' · ')
  return (
    <BackgroundCard
      title="To-do"
      color={backgroundColors.todos.bg}
      summary={text ?? [counts, tags, !expanded && now].filter(Boolean).join(' · ')}
      expanded={expanded}
      onExpanded={(open) => store.getState().setTodosExpanded(session, open)}
    >
      {text ? <p role="status">{text}</p> : (
        <ul className="wrap-anywhere">
          {items.map((t, i) => (
            <li key={i} className={cn('flex gap-3 py-1', (t.status === 'done' || t.status === 'dropped') && 'text-fg-tertiary')}>
              <span className="w-20 shrink-0 pt-px text-xs text-fg-tertiary">[{LABEL[t.status]}]</span>
              <span className="min-w-0 flex-1">
                {t.text}
                {t.reason && <span className="block text-fg-secondary">{t.reason}</span>}
              </span>
            </li>
          ))}
        </ul>
      )}
    </BackgroundCard>
  )
}
