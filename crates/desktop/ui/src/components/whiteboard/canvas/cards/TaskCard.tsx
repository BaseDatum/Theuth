// The `task` card: a task's live record (the task store's list, refetched as tasks change: `useLiveTasks`) — state,
// title, priority, owner, open blockers, the first line of its done-when — and, gone from the store, "task deleted".
// Its toolbar items (Open in pane, the detail page's legal moves) and the sync that keeps the board's blocker links
// and card titles in step with the tasks live here too; the pure rules are `task-cards.ts`.

import { useRouter } from '@tanstack/react-router'
import { CircleCheck, Hourglass, PanelRight } from 'lucide-react'
import { useEffect, useMemo } from 'react'
import type * as Y from 'yjs'
import { doneBecause, EMPTY_FILTER, useCloseDone, useLiveTasks, useMoveTask, useTasks } from '@/components/tasks/queries'
import { ownerLabel, shortId, stateChip } from '@/components/tasks/helpers'
import { STATE_TONE } from '@/components/tasks/graph/model'
import type { Task } from '@/rpc/bindings/method.roster.output'
import { openTaskPane, syncTaskCards, type TaskLite, taskMoves, taskOf } from '../../task-cards'
import { useCanvas } from '../context'
import type { Board, Card } from '../model'

export type TaskRecord = { status: 'loading' } | { status: 'error'; message: string } | { status: 'deleted' } | { status: 'live'; task: Task }

/** Task `id` from the store's full list: loading, unavailable, deleted (the list has no such task), or live. */
export function useTaskRecord(id: string): TaskRecord {
  const list = useTasks(EMPTY_FILTER)
  if (list.isError) return { status: 'error', message: list.error instanceof Error ? list.error.message : String(list.error) }
  if (!list.data) return { status: 'loading' }
  const task = list.data.find((t) => t.id === id)
  return task ? { status: 'live', task } : { status: 'deleted' }
}

const PRIORITY_TONE: Record<Task['priority'], string | null> = {
  urgent: 'var(--color-danger)',
  high: 'var(--color-warning)',
  medium: null,
  low: 'var(--color-fg-tertiary)',
}

/** Open blockers: those the engine has not marked resolved. */
const openBlockers = (t: Task): number => t.blockers.filter((b) => !t.resolved_blockers.includes(b)).length

/**
 * The card's body. Laid out at its natural size (280 px wide, as tall as its lines), so size-to-content measures it;
 * it never takes the pointer on the board (CardNode's shield and click-to-open do).
 */
export default function TaskCardBody({ task: id }: { task: string }) {
  const r = useTaskRecord(id)
  if (r.status !== 'live') {
    const text = r.status === 'loading' ? 'Loading task…' : r.status === 'deleted' ? 'task deleted' : `tasks unavailable — ${r.message}`
    return (
      <div className={`flex w-[280px] flex-col gap-1 p-3 text-[12px] ${r.status === 'deleted' ? 'opacity-50' : ''}`} data-task-card={id} data-task-status={r.status}>
        <span className="font-medium text-fg-secondary">{text}</span>
        <span className="font-mono text-[11px] text-fg-tertiary">#{shortId(id)}</span>
      </div>
    )
  }
  const t = r.task
  const tone = STATE_TONE[t.state]
  const prio = PRIORITY_TONE[t.priority]
  const blockers = openBlockers(t)
  const doneWhen = t.done_when?.trim().split('\n')[0]?.trim()
  const closed = t.state === 'done' || t.state === 'cancelled'
  return (
    <div className={`flex w-[280px] flex-col gap-1.5 p-3 text-[12px] leading-4 ${closed ? 'opacity-70' : ''}`} data-task-card={id} data-task-status="live" data-task-state={t.state}>
      <div className="flex min-w-0 items-center gap-1.5">
        <span
          className="max-w-full shrink-0 truncate rounded-full border px-1.5 text-[11px] font-medium"
          style={{ color: tone, borderColor: `color-mix(in oklab, ${tone} 45%, transparent)`, background: `color-mix(in oklab, ${tone} 12%, transparent)` }}
          data-task-badge
        >
          {stateChip(t)}
        </span>
        {blockers > 0 && (
          <span className="flex shrink-0 items-center gap-0.5 text-danger" data-task-blockers={blockers}>
            <Hourglass className="size-3" aria-hidden />
            {blockers} {blockers === 1 ? 'blocker' : 'blockers'}
          </span>
        )}
      </div>
      <p className={`line-clamp-3 text-[13px] leading-[17px] font-medium wrap-anywhere text-foreground ${t.state === 'cancelled' ? 'line-through' : ''}`} data-task-title>
        {t.title}
      </p>
      <div className="flex min-w-0 items-center gap-1.5 text-[11px] text-fg-secondary">
        <span className="min-w-0 truncate" data-task-owner>
          {ownerLabel(t.owner)}
        </span>
        <span aria-hidden>·</span>
        <span className="shrink-0" style={prio ? { color: prio } : undefined} data-task-priority>
          {t.priority}
        </span>
        <span className="ml-auto shrink-0 font-mono text-fg-tertiary">#{shortId(t.id)}</span>
      </div>
      {doneWhen && (
        <p className="flex min-w-0 items-start gap-1 text-[11px] text-fg-secondary" data-task-done-when>
          <CircleCheck className="mt-0.5 size-3 shrink-0 text-success" aria-label="Done when" />
          <span className="line-clamp-2 wrap-anywhere">{doneWhen}</span>
        </p>
      )}
    </div>
  )
}

/** Open the task in a pane beside the whiteboard (a phone: its page), through the app's router when there is one. */
export function useOpenTask(): (task: string, title?: string) => void {
  const router = useRouter({ warn: false })
  return (task, title) => void openTaskPane(task, title, router ? (href) => void router.navigate({ href }) : undefined)
}

/** The task card's toolbar items: Open in pane, then the moves its detail page allows (`taskMoves`). */
export function TaskTools({ card }: { card: Card }) {
  const id = taskOf(card)
  const { readOnly } = useCanvas()
  const r = useTaskRecord(id ?? '')
  const open = useOpenTask()
  const move = useMoveTask()
  const close = useCloseDone()
  if (!id) return null
  const busy = move.isPending || close.isPending
  return (
    <>
      <button type="button" onClick={() => open(id, card.title)}>
        <PanelRight className="size-4" /> Open in pane
      </button>
      {!readOnly && r.status === 'live' && taskMoves(r.task).map((m) => (
        <button
          key={m.label}
          type="button"
          disabled={busy}
          data-task-move={m.to}
          onClick={() => (m.done ? close.mutate({ id, because: doneBecause(r.task) }) : move.mutate({ id, state: m.to }))}
        >
          {m.label}
        </button>
      ))}
    </>
  )
}

/**
 * Keeps the board's task cards in step with the tasks (owner, writable boards only): blocker links added and removed
 * (`syncTaskCards`, sync links only) and card titles following their task's. Mounted only while the board has a task
 * card, so a board without one never fetches tasks.
 */
export function TaskSync({ doc, board }: { doc: Y.Doc; board: Board }) {
  useLiveTasks()
  const list = useTasks(EMPTY_FILTER)
  const tasks = useMemo(() => (list.data ? new Map<string, TaskLite>(list.data.map((t) => [t.id, t])) : null), [list.data])
  useEffect(() => {
    if (tasks) syncTaskCards(doc, board, tasks)
  }, [doc, board, tasks])
  return null
}
