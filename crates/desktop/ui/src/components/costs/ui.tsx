// 160 §7: the costs page's small shared pieces — the card with its mono title, the segmented control, tags, swatches.
// Theme tokens only.

import type { ReactNode } from 'react'
import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip'
import { cn } from '@/lib/utils'

/** A card: 5 px square + mono uppercase title, an optional muted note and actions on the right. */
export function Panel({ title, note, actions, className, children, id }: { title: ReactNode; note?: ReactNode; actions?: ReactNode; className?: string; children: ReactNode; id?: string }) {
  return (
    <section id={id} className={cn('min-w-0 rounded-xl border border-border bg-card px-3.5 py-3', className)}>
      <header className="mb-2.5 flex flex-wrap items-center gap-2">
        <h3 className="flex items-center gap-[7px] font-mono text-[10px] font-medium tracking-[1.4px] text-fg-secondary uppercase">
          <span className="inline-block size-[5px] bg-fg-tertiary" />
          {title}
        </h3>
        <div className="flex-1" />
        {note && <span className="text-[11px] text-fg-tertiary">{note}</span>}
        {actions}
      </header>
      {children}
    </section>
  )
}

export type SegItem<T extends string> = { value: T; label: ReactNode; /** Why it is disabled; `null`/absent = enabled. */ blocked?: string | null }

/** The segmented control: a well with one raised choice; a blocked choice is dimmed and explains itself in a tooltip. */
export function Seg<T extends string>({ items, value, onChange, label }: { items: SegItem<T>[]; value: T | null; onChange: (v: T) => void; label: string }) {
  return (
    <div role="radiogroup" aria-label={label} className="inline-flex max-w-full gap-0.5 overflow-x-auto rounded-lg bg-layer-2 p-0.5 [scrollbar-width:none]">
      {items.map((it) => {
        const on = it.value === value
        const b = (
          <button
            key={it.value}
            type="button"
            role="radio"
            aria-checked={on}
            aria-disabled={it.blocked ? true : undefined}
            onClick={() => !it.blocked && onChange(it.value)}
            className={cn(
              'shrink-0 rounded-md px-2.5 py-1 font-mono text-[11px] text-fg-tertiary outline-none hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring/50',
              on && 'bg-card text-foreground shadow-[0_0_0_1px_var(--border-strong)]',
              it.blocked && 'cursor-default opacity-35 hover:text-fg-tertiary',
            )}
          >
            {it.label}
          </button>
        )
        return it.blocked ? (
          <Tooltip key={it.value}>
            <TooltipTrigger asChild>{b}</TooltipTrigger>
            <TooltipContent>{it.blocked}</TooltipContent>
          </Tooltip>
        ) : (
          b
        )
      })}
    </div>
  )
}

export type Tone = 'ok' | 'warn' | 'danger' | 'info' | 'muted'
const TONE: Record<Tone, string> = {
  ok: 'text-success border-success/50',
  warn: 'text-warning border-warning/50',
  danger: 'text-danger border-danger/50',
  info: 'text-info border-info/50',
  muted: 'text-fg-tertiary border-border-strong',
}

export function Tag({ tone = 'muted', children, className }: { tone?: Tone; children: ReactNode; className?: string }) {
  return <span className={cn('inline-flex shrink-0 items-center rounded-[5px] border px-1.5 py-px font-mono text-[10px] whitespace-nowrap', TONE[tone], className)}>{children}</span>
}

export function Swatch({ color, className }: { color: string; className?: string }) {
  return <i aria-hidden className={cn('inline-block size-[9px] shrink-0 rounded-[2px]', className)} style={{ background: color }} />
}

/** The owner glyph of the HUD: ring = agent, square = routine, nothing for you/other. */
export function OwnerGlyph({ kind }: { kind: string }) {
  if (kind === 'agent') return <span aria-hidden className="inline-block size-[7px] shrink-0 rounded-full border border-fg-secondary" />
  if (kind === 'routine') return <span aria-hidden className="inline-block size-1.5 shrink-0 bg-fg-secondary" />
  return null
}
