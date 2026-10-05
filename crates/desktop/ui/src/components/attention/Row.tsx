import { Link } from '@tanstack/react-router'
import { KIND_LABEL } from '@/components/hud/Hud'
import { age } from '@/components/tasks/helpers'
import { cn } from '@/lib/utils'
import type { AttentionItem } from '@/rpc/bindings/AttentionItem'

/** One rail row (101 D6): kind chip · title (two lines) · `by <agent> · <age>`. No buttons — the screen acts. */
export function Row({ item, active }: { item: AttentionItem; active: boolean }) {
  return (
    <li>
      <Link
        to="/attention/$id"
        params={{ id: item.id }}
        aria-current={active || undefined}
        className={cn('flex flex-col gap-1 px-3 py-2.5 text-[13px] transition-colors hover:bg-accent/60', active && 'bg-accent')}
      >
        <span className="w-fit rounded-full bg-layer-2 px-2 py-0.5 text-[11px] uppercase tracking-wide text-fg-secondary">{KIND_LABEL[item.kind]}</span>
        <p className="line-clamp-2 font-medium">{item.title}</p>
        <p className="truncate text-xs text-fg-tertiary tabular-nums">
          {item.agent && `by ${item.agent} · `}
          {age(item.at_ms)}
        </p>
      </Link>
    </li>
  )
}
