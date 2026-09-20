// 05 §7: one cached brief — title, bullets, alias and tag chips, age, uses, edit / delete.

import { Pencil, Trash2 } from 'lucide-react'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { cn } from '@/lib/utils'
import type { CachedBrief } from '@/rpc/bindings/CachedBrief'

/** `3d` / `2h` / `11m`, as the batch line renders it (session.rs `age`). */
export function age(iso: string, now = Date.now()): string {
  const mins = Math.max(0, Math.floor((now - new Date(iso).getTime()) / 60_000))
  if (Number.isNaN(mins)) return '?'
  if (mins >= 24 * 60) return `${Math.floor(mins / (24 * 60))}d`
  if (mins >= 60) return `${Math.floor(mins / 60)}h`
  return `${mins}m`
}

export function BriefCard({
  brief,
  highlight,
  onEdit,
  onDelete,
  onTag,
}: {
  brief: CachedBrief
  /** `?q=`/`#id` target: outlined. */
  highlight?: boolean
  onEdit: () => void
  onDelete: () => void
  onTag?: (tag: string) => void
}) {
  return (
    <div
      id={brief.id}
      className={cn('rounded-xl border border-border-subtle p-3 text-[13px]', highlight && 'border-fg-accent')}
    >
      <div className="flex items-baseline gap-2">
        <span className="min-w-0 flex-1 truncate font-semibold">{brief.title}</span>
        <span className="shrink-0 text-[11px] text-fg-tertiary" title={`updated ${new Date(brief.updated_at).toLocaleString()} · ${brief.source}`}>
          {age(brief.updated_at)} · used {brief.uses}×
        </span>
        <Button variant="ghost" size="icon-sm" aria-label="Edit brief" onClick={onEdit}>
          <Pencil className="size-3.5" />
        </Button>
        <Button variant="ghost" size="icon-sm" aria-label="Delete brief" onClick={onDelete}>
          <Trash2 className="size-3.5" />
        </Button>
      </div>
      <ul className="mt-1 list-disc space-y-0.5 pl-4 text-fg-secondary">
        {brief.bullets.map((b) => (
          <li key={b}>{b}</li>
        ))}
      </ul>
      <div className="mt-2 flex flex-wrap items-center gap-1">
        {brief.aliases.map((a) => (
          <Badge key={a} variant="secondary" className="text-[10px]">
            {a}
          </Badge>
        ))}
        {brief.tags.map((t) => (
          <Badge
            key={t}
            variant="outline"
            className={cn('text-[10px]', onTag && 'cursor-pointer hover:bg-layer-2')}
            onClick={onTag ? () => onTag(t) : undefined}
          >
            #{t}
          </Badge>
        ))}
      </div>
    </div>
  )
}
