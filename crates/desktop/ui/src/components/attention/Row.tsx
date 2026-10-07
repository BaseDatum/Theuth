import { Link } from '@tanstack/react-router'
import { title } from '@/components/chat/broadcast'
import { DoubleChip } from '@/components/chat/DoubleChip'
import { doubleIds } from '@/components/hud/Hud'
import { KIND_LABEL } from '@/components/hud/Hud'
import { age } from '@/components/tasks/helpers'
import { cn } from '@/lib/utils'
import type { AttentionItem } from '@/rpc/bindings/AttentionItem'
import type { SessionSummary } from '@/rpc/bindings/SessionSummary'

/** One rail row (101 D6, 104 A5): kind chip · title (two lines) · `from <session title> · <agent> · <age>`, unknown parts left out. No buttons — the screen acts. 126: the double's chip after the kind chip. */
export function Row({ item, active, sessions }: { item: AttentionItem; active: boolean; sessions: readonly SessionSummary[] | undefined }) {
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
          <span className="w-fit rounded-full bg-layer-2 px-2 py-0.5 text-[11px] uppercase tracking-wide text-fg-secondary">{KIND_LABEL[item.kind]}</span>
          <DoubleChip ids={doubleIds(item)} />
        </span>
        <p className="line-clamp-2 font-medium">{item.title}</p>
        {/* 130 E3: a finished turn's one-line summary. */}
        {item.summary && <p className="truncate text-xs text-fg-secondary">{item.summary}</p>}
        <p className="truncate text-xs text-fg-tertiary tabular-nums">
          {[from && `from ${from}`, item.agent, age(item.at_ms)].filter(Boolean).join(' · ')}
        </p>
      </Link>
    </li>
  )
}
