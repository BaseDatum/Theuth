import { Link } from '@tanstack/react-router'
import { title } from '@/components/chat/broadcast'
import { DoubleChip } from '@/components/chat/DoubleChip'
import { doubleIds, itemLabel } from '@/components/hud/Hud'
import { age } from '@/components/tasks/helpers'
import { itemHome } from '@/components/tasks/home'
import { useMesh, useTasks } from '@/components/tasks/queries'
import { cn } from '@/lib/utils'
import type { AttentionItem } from '@/rpc/bindings/AttentionItem'
import type { SessionSummary } from '@/rpc/bindings/SessionSummary'

/** 132 D1/D4: `urgent` and `high` show a chip after the kind chip (gold-ochre · the warning hue); medium/low show nothing. */
export function PriorityChip({ priority }: { priority: AttentionItem['priority'] | undefined }) {
  if (priority !== 'urgent' && priority !== 'high') return null
  return <span className={cn('w-fit rounded-full border px-2 py-0.5 text-[11px] uppercase tracking-wide', priority === 'urgent' ? 'border-fg-accent text-fg-accent' : 'border-warning text-warning')}>{priority}</span>
}

/**
 * One rail row (101 D6, 104 A5): kind chip · title (two lines) · `from <session title> · <agent> · <age>`, unknown parts left out. No buttons — the screen acts. 126: the double's chip after the kind chip.
 * 132 D1: the engine's `title` is the ask (decision) or the outcome (review) and `detail` the byline, shown as one secondary line for those two kinds; nothing is composed here.
 */
export function Row({ item, active, sessions }: { item: AttentionItem; active: boolean; sessions: readonly SessionSummary[] | undefined }) {
  const on = itemHome(item.task, useTasks().data, useMesh().data)
  const from = item.session ? title(item.session, sessions, item.session.slice(0, 8)) : ''
  return (
    <li>
      <Link
        to="/attention/$id"
        params={{ id: item.id }}
        search={true}
        aria-current={active || undefined}
        className={cn('flex min-h-11 flex-col gap-1 px-3 py-2.5 text-[13px] transition-colors hover:bg-accent/60', active && 'bg-accent')}
      >
        <span className="flex flex-wrap items-center gap-1.5">
          <span className="w-fit rounded-full bg-layer-2 px-2 py-0.5 text-[11px] uppercase tracking-wide text-fg-secondary">{itemLabel(item)}</span>
          <PriorityChip priority={item.priority} />
          {/* 155 §7.5: done ≠ idle — the engine keeps `alert` on a finished item until this connection has looked at the session. */}
          {item.alert?.class === 'finished' && <span className="w-fit rounded-full border border-success-border px-2 py-0.5 text-[11px] uppercase tracking-wide text-success">done</span>}
          <DoubleChip ids={doubleIds(item)} />
        </span>
        <p className="line-clamp-2 font-medium">{item.title}</p>
        {item.detail && (item.kind === 'decision' || item.kind === 'review') && <p className="truncate text-xs text-fg-secondary">{item.detail}</p>}
        {/* 130 E3: a finished turn's one-line summary. */}
        {item.summary && <p className="truncate text-xs text-fg-secondary">{item.summary}</p>}
        <p className="truncate text-xs text-fg-tertiary tabular-nums">
          {[from && `from ${from}`, item.agent, on, age(item.at_ms)].filter(Boolean).join(' · ')}
        </p>
      </Link>
    </li>
  )
}
