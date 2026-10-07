// 101: what the three screens share — the D11 "move on" hook, the kind chip, the scrolling body, the footer actions.
import { useQueryClient } from '@tanstack/react-query'
import { Link, useNavigate } from '@tanstack/react-router'
import type { ReactNode } from 'react'
import { KIND_LABEL } from '@/components/hud/Hud'
import { snoozeUntil } from '@/components/hud/model'
import { ATTENTION_KEY, useAttentionActions } from '@/components/hud/queries'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { useIsMobile } from '@/hooks/use-mobile'
import { cn } from '@/lib/utils'
import type { AttentionItem } from '@/rpc/bindings/AttentionItem'
import type { Packet } from '@/rpc/bindings/Packet'
import type { Priority } from '@/rpc/bindings/Priority'
import { nextAfter } from './model'

/**
 * 101 D11: where the pane goes once `item` left the list — desktop: the next newest item (the list in cache still holds
 * the resolved one at this moment; the refetch is in flight), or `/attention`; phones: back to the list.
 */
export function useResolved(item: AttentionItem): () => void {
  const navigate = useNavigate()
  const qc = useQueryClient()
  const mobile = useIsMobile()
  return () => {
    const next = mobile ? null : nextAfter(qc.getQueryData<AttentionItem[]>(ATTENTION_KEY) ?? [], item.id)
    // `search: true` keeps `?sim=` / `?mock=` alive in dev; in the app the route carries no search.
    void (next ? navigate({ to: '/attention/$id', params: { id: next.id }, search: true }) : navigate({ to: '/attention', search: true }))
  }
}

export function Chip({ kind }: { kind: AttentionItem['kind'] }) {
  return <span className="rounded-full bg-layer-2 px-2 py-0.5 text-[11px] uppercase tracking-wide text-fg-secondary">{KIND_LABEL[kind]}</span>
}

/** The scrolling column every screen shares: 720 px of content, padded. A size container so `PacketBoard.css` can use its width (`cqi`) and visible height (`cqh`). */
export function Body({ children }: { children: ReactNode }) {
  return (
    <div className="min-h-0 flex-1 overflow-y-auto [container-type:size]">
      <div className="mx-auto flex w-full max-w-[720px] flex-col gap-6 px-4 py-5 md:px-6">{children}</div>
    </div>
  )
}

export function SectionTitle({ children }: { children: ReactNode }) {
  return <h3 className="text-xs font-semibold tracking-wide text-fg-tertiary uppercase">{children}</h3>
}

/** Open task · Snooze · Dismiss — the footer of the decision and review screens (`onResolved` after Dismiss). */
export function Footer({ item, onResolved, children }: { item: AttentionItem; onResolved: () => void; children?: ReactNode }) {
  const act = useAttentionActions()
  const busy = act.snooze.isPending || act.dismiss.isPending
  return (
    <div className="flex flex-wrap items-center gap-1.5">
      {children}
      {item.task && (
        <Button asChild variant="ghost" size="sm">
          <Link to="/tasks/$id" params={{ id: item.task }}>
            Open task
          </Link>
        </Button>
      )}
      <Button variant="ghost" size="sm" disabled={busy} title="Snooze until 09:00 tomorrow" onClick={() => void act.snooze.mutateAsync({ id: item.id, until: snoozeUntil('tomorrow') }).then(onResolved, () => undefined)}>
        Snooze
      </Button>
      <Button variant="ghost" size="sm" disabled={busy} onClick={() => void act.dismiss.mutateAsync(item.id).then(onResolved, () => undefined)}>
        Dismiss
      </Button>
    </div>
  )
}

/** 132 D2: `Packet.if_wrong` — the one honest line on the cost of a wrong answer, above the options (decision) or the bottom bar (review). */
export function IfWrong({ text }: { text: string | null | undefined }) {
  if (!text) return null
  return (
    <p className="rounded-md border border-border-subtle bg-layer-2 px-3 py-2 text-sm">
      <span className="font-medium text-fg-secondary">If wrong — </span>
      {text}
    </p>
  )
}

/**
 * The option cards of a decision (101 D8) and of a review's next moves (132 D3): ○/● · label · `recommended` badge ·
 * description; `picked` is the lit index (`null` = none). Full width, so they wrap rather than overflow at 390 px.
 */
export function OptionCards({ options, picked, recommended, onPick }: { options: Packet['options']; picked: number | null; recommended: number | null | undefined; onPick: (i: number) => void }) {
  return options.map((c, i) => {
    const on = picked === i
    return (
      <button
        key={c.label}
        type="button"
        aria-pressed={on}
        onClick={() => onPick(i)}
        className={cn('flex w-full items-start gap-2 rounded-md border px-3 py-2 text-left text-sm transition-colors', on ? 'border-fg-accent bg-accent' : 'border-border-subtle hover:bg-accent/60')}
      >
        <span className="mt-0.5 text-xs text-fg-tertiary">{on ? '●' : '○'}</span>
        <span className="flex min-w-0 flex-1 flex-col">
          <span className="flex items-center gap-2">
            {c.label}
            {recommended === i && (
              <Badge variant="secondary" className="px-1.5 py-0 text-[10px]">
                recommended
              </Badge>
            )}
          </span>
          {c.description && <span className="text-xs text-fg-secondary">{c.description}</span>}
        </span>
      </button>
    )
  })
}

export const PRIORITIES: readonly Priority[] = ['urgent', 'high', 'medium', 'low']

/**
 * 132 D4: the task's priority, the user's to set — `task_update(task, { priority })`, then the inbox refetches (the
 * engine re-ranks). Only task-born items have one; the control renders nothing otherwise. Sits in the Footer row and
 * wraps with it at 390 px.
 */
export function PrioritySelect({ item }: { item: AttentionItem }) {
  const act = useAttentionActions()
  if (!item.task) return null
  const task = item.task
  return (
    <Select value={item.priority ?? 'medium'} onValueChange={(v) => act.priority.mutate({ id: task, priority: v as Priority })} disabled={act.priority.isPending}>
      <SelectTrigger size="sm" aria-label="Priority" className="h-8 gap-1 border-0 bg-transparent px-3 text-xs shadow-none hover:bg-accent/60 dark:bg-transparent">
        <span className="text-fg-tertiary">Priority</span>
        <SelectValue />
      </SelectTrigger>
      <SelectContent>
        {PRIORITIES.map((p) => (
          <SelectItem key={p} value={p}>
            {p}
          </SelectItem>
        ))}
      </SelectContent>
    </Select>
  )
}
