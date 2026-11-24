import type { ReactNode } from 'react'
import { DialogDescription, DialogTitle } from '@/components/ui/dialog'

/**
 * One wizard step: heading (the dialog's title and description, so a screen reader announces each step), a body that
 * scrolls, a side panel that shows what the step means (from `lg` up), and a footer with the quiet way out on the
 * left and the one primary action on the right.
 */
export function StepFrame({ eyebrow, title, description, children, aside, back, primary }: { eyebrow?: string; title: string; description: ReactNode; children: ReactNode; aside?: ReactNode; back?: ReactNode; primary?: ReactNode }) {
  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div className="flex min-h-0 flex-1 gap-8 px-6 pb-6 sm:px-12 lg:pr-6">
        <div className="min-h-0 min-w-0 flex-1 overflow-y-auto pt-2 pb-2">
          <header className="mb-8 flex flex-col gap-2">
            {eyebrow && <p className="text-xs font-medium tracking-[0.08em] text-fg-accent uppercase">{eyebrow}</p>}
            <DialogTitle className="text-2xl leading-tight font-semibold tracking-tight text-foreground sm:text-[28px]">{title}</DialogTitle>
            <DialogDescription className="text-[15px] leading-relaxed text-fg-secondary">{description}</DialogDescription>
          </header>
          {children}
        </div>
        {aside && (
          <aside aria-hidden className="hidden w-[300px] shrink-0 flex-col justify-center overflow-hidden rounded-2xl border border-border-subtle bg-layer-2 p-7 lg:flex">
            {aside}
          </aside>
        )}
      </div>
      <footer className="flex shrink-0 items-center justify-between gap-3 border-t border-border-subtle bg-layer-1 px-6 py-4 sm:px-12">
        <div className="flex items-center gap-2">{back}</div>
        <div data-primary className="flex items-center gap-3">{primary}</div>
      </footer>
    </div>
  )
}

/** The selectable card every step uses: a calm surface that lifts on hover and shows the focus ring and the choice. */
export const CARD =
  'rounded-xl border border-border-subtle bg-layer-2 text-left transition-[border-color,background-color,box-shadow] duration-150 hover:border-border-strong hover:bg-layer-3 focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none motion-reduce:transition-none'

/** A side panel's heading and its short list of facts. */
export function Facts({ icon, title, facts }: { icon: ReactNode; title: string; facts: readonly string[] }) {
  return (
    <div className="flex flex-col gap-5">
      <span className="flex size-10 items-center justify-center rounded-lg bg-layer-1 text-fg-accent">{icon}</span>
      <p className="text-[17px] leading-snug font-medium text-foreground">{title}</p>
      <ul className="flex flex-col gap-3.5">
        {facts.map((f, i) => (
          <li key={f} className="onb-rise flex gap-2.5 text-[13px] leading-relaxed text-fg-secondary" style={{ animationDelay: `${120 + 80 * i}ms` }}>
            <span className="mt-[7px] size-1.5 shrink-0 rounded-full bg-fg-accent" />
            {f}
          </li>
        ))}
      </ul>
    </div>
  )
}
