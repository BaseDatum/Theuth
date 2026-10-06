import { Link } from '@tanstack/react-router'
import { ChevronRight, CircleAlert, Eye, UserPlus } from 'lucide-react'
import { useState } from 'react'
import { CollapsibleTrigger } from '@/components/ui/collapsible'
import type { AgentSummary } from '@/rpc/bindings/AgentSummary'
import type { Task } from '@/rpc/bindings/Task'
import { awaitsMyReview, needsPerson, ownerLabel, queueSummary } from './helpers'
import { useAgents, useTasks } from './queries'

/**
 * "Does it need me" (67 §12): blocked tasks waiting on a decision or a person, reviews where you are the reviewer,
 * proposed agents. 115 split it in two so the board page can scroll past the list: `QueueBar` is the one pinned
 * header row (the Radix `CollapsibleTrigger`), `QueueList` the dense rows (one per item, one click each) rendered
 * inside the page's vertical scroller. `Board` owns the `Collapsible` root and the persisted open state.
 */
export type QueueItems = { decisions: Task[]; reviews: Task[]; proposed: AgentSummary[]; total: number }

/** The two queries behind the queue and their filters; empty arrays while the engine cannot answer yet. */
export function useQueue(): QueueItems {
  const tasks = useTasks({ owner: null, state: ['blocked', 'review'], engine: null, project: null })
  const agents = useAgents()
  const decisions = tasks.data?.filter(needsPerson) ?? []
  const reviews = tasks.data?.filter(awaitsMyReview) ?? []
  const proposed = agents.data?.filter((a) => a.status === 'proposed' || a.file.proposed_by) ?? []
  return { decisions, reviews, proposed, total: decisions.length + reviews.length + proposed.length }
}

/** 115: the bar's open/closed choice is per device; `'1'` = open, anything else (a fresh profile included) = closed. */
export const QUEUE_OPEN_KEY = 'theuth_tasks_queue_open'

export function useQueueOpen(): [boolean, (open: boolean) => void] {
  const [open, setOpen] = useState(() => {
    try {
      return localStorage.getItem(QUEUE_OPEN_KEY) === '1'
    } catch {
      return false
    }
  })
  return [
    open,
    (v: boolean) => {
      setOpen(v)
      try {
        localStorage.setItem(QUEUE_OPEN_KEY, v ? '1' : '0')
      } catch {
        // private mode / storage denied: the choice lasts the session
      }
    },
  ]
}

/**
 * The pinned header row: `▸ 4 need you · 2 decisions · 1 review · 1 proposed agent`. Must sit inside `Board`'s
 * `Collapsible` root (Radix adds aria-expanded/aria-controls and `data-state`, which turns the chevron). Nothing at all
 * when the queue is empty.
 */
export function QueueBar({ items }: { items: QueueItems }) {
  if (items.total === 0) return null
  const { headline, breakdown } = queueSummary({ decisions: items.decisions.length, reviews: items.reviews.length, proposed: items.proposed.length })
  return (
    <CollapsibleTrigger asChild>
      <button
        type="button"
        className="group flex h-8 w-full shrink-0 items-center gap-2 border-b border-border-subtle bg-layer-1 px-3 text-left text-[13px] outline-none hover:bg-layer-2 focus-visible:ring-2 focus-visible:ring-ring/50 focus-visible:ring-inset"
      >
        <ChevronRight className="size-3.5 shrink-0 text-fg-tertiary transition-transform group-data-[state=open]:rotate-90" />
        <span className="shrink-0 text-foreground">{headline}</span>
        <span className="min-w-0 truncate text-fg-tertiary">· {breakdown}</span>
      </button>
    </CollapsibleTrigger>
  )
}

/** The rows, exactly as before 115: decisions, then reviews, then proposed agents; 32 px each, one link each. */
export function QueueList({ items }: { items: QueueItems }) {
  if (items.total === 0) return null
  return (
    <ul className="flex shrink-0 flex-col divide-y divide-border-subtle border-b border-border-subtle bg-layer-1 text-[13px]">
      {items.decisions.map((t) => (
        <Item key={t.id} to={t.id} icon={<CircleAlert className="size-3.5 text-danger" />} label={t.title} meta={`${ownerLabel(t.owner)} · ${t.blockers[0]}`} />
      ))}
      {items.reviews.map((t) => (
        <Item key={t.id} to={t.id} icon={<Eye className="size-3.5 text-warning" />} label={t.title} meta={`review · ${ownerLabel(t.owner)}`} />
      ))}
      {items.proposed.map((a) => (
        <li key={a.file.name}>
          <Link to="/agents/$name" params={{ name: a.file.name }} className="flex h-8 items-center gap-2 px-3 hover:bg-layer-2">
            <UserPlus className="size-3.5 text-primary" />
            <span className="min-w-0 flex-1 truncate">
              Approve <b>{a.file.title}</b>
            </span>
            <span className="truncate text-fg-tertiary">proposed by {a.file.proposed_by ?? '—'}</span>
          </Link>
        </li>
      ))}
    </ul>
  )
}

function Item({ to, icon, label, meta }: { to: string; icon: React.ReactNode; label: string; meta: string }) {
  return (
    <li>
      <Link to="/tasks/$id" params={{ id: to }} className="flex h-8 items-center gap-2 px-3 hover:bg-layer-2">
        {icon}
        <span className="min-w-0 flex-1 truncate">{label}</span>
        <span className="truncate text-fg-tertiary">{meta}</span>
      </Link>
    </li>
  )
}
