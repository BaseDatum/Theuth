// 160 §7: the costs page's small shared pieces — the card with its mono title, the segmented control, tags, swatches.
// Theme tokens only.

import { type ReactNode, startTransition, useEffect, useRef, useState } from 'react'
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

/**
 * The segmented control: a well with one raised choice; a blocked choice is dimmed and explains itself in a tooltip.
 * It never widens the page: past its container it scrolls sideways, and `fade` softens the right edge on phones.
 */
export function Seg<T extends string>({ items, value, onChange, label, fade = false }: { items: SegItem<T>[]; value: T | null; onChange: (v: T) => void; label: string; fade?: boolean }) {
  return (
    <div
      role="radiogroup"
      aria-label={label}
      className={cn('inline-flex max-w-full gap-0.5 overflow-x-auto rounded-lg bg-layer-2 p-0.5 [scrollbar-width:none]', fade && 'max-md:pr-6 max-md:[mask-image:linear-gradient(to_right,black_calc(100%-28px),transparent)]')}
    >
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

/**
 * `v`, taken `stage` frames after it changes (stage 0: at once), in a transition — splits one view change into several
 * frame-sized commits instead of one long one (§6.5).
 */
export function useStaged<T>(v: T, stage: number): T {
  const [held, setHeld] = useState(v)
  useEffect(() => {
    if (stage === 0 || held === v) return
    const t = setTimeout(() => startTransition(() => setHeld(v)), stage * 16)
    return () => clearTimeout(t)
  }, [v, held, stage])
  return stage === 0 ? v : held
}

const idle = (fn: () => void): number => (typeof requestIdleCallback === 'function' ? requestIdleCallback(fn, { timeout: 400 }) : window.setTimeout(fn, 16))
const unidle = (id: number) => (typeof cancelIdleCallback === 'function' ? cancelIdleCallback(id) : clearTimeout(id))

/**
 * Below-the-fold mounting (160 R2): a blank block of `height` until its turn — the `order`-th idle slice after the page
 * mounts, or earlier when a scroll brings it within half a viewport — then its children, in a transition. The block
 * holds the section's usual height so nothing above it moves. A server render (tests) mounts at once.
 */
export function Later({ order, height, children }: { order: number; height: number; children: ReactNode }) {
  const [on, setOn] = useState(typeof window === 'undefined')
  const ref = useRef<HTMLDivElement>(null)
  useEffect(() => {
    const el = ref.current
    if (on || !el) return
    let live = true
    const go = () => live && startTransition(() => setOn(true))
    // The first callback reports the starting layout; only a later one (the user scrolled) mounts early.
    let first = true
    const io = new IntersectionObserver(
      (es) => {
        if (!first && es.some((e) => e.isIntersecting)) go()
        first = false
      },
      { root: el.closest('[data-costs-scroll]'), rootMargin: '50% 0px' },
    )
    io.observe(el)
    let n = order
    const tick = () => (--n <= 0 ? go() : (id = idle(tick)))
    let id = idle(tick)
    return () => {
      live = false
      io.disconnect()
      unidle(id)
    }
  }, [on, order])
  return on ? children : <div ref={ref} aria-hidden style={{ height }} />
}
