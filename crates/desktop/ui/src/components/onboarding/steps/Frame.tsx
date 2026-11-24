import type { ReactNode } from 'react'
import { DialogDescription, DialogTitle } from '@/components/ui/dialog'

/**
 * One wizard step: heading (the dialog's title and description, so a screen reader announces each step), a body that
 * scrolls, and a footer with the quiet way out on the left and the one primary action on the right.
 */
export function StepFrame({ eyebrow, title, description, children, back, primary }: { eyebrow: string; title: string; description: ReactNode; children: ReactNode; back?: ReactNode; primary?: ReactNode }) {
  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div className="min-h-0 flex-1 overflow-y-auto px-6 pt-2 pb-8 sm:px-12">
        <header className="mb-8 flex max-w-xl flex-col gap-2">
          <p className="text-xs font-medium tracking-[0.08em] text-fg-accent uppercase">{eyebrow}</p>
          <DialogTitle className="text-2xl leading-tight font-semibold tracking-tight text-foreground sm:text-[28px]">{title}</DialogTitle>
          <DialogDescription className="text-[15px] leading-relaxed text-fg-secondary">{description}</DialogDescription>
        </header>
        {children}
      </div>
      <footer className="flex shrink-0 items-center justify-between gap-3 border-t border-border-subtle bg-layer-1 px-6 py-4 sm:px-12">
        <div className="flex items-center gap-2">{back}</div>
        <div className="flex items-center gap-3">{primary}</div>
      </footer>
    </div>
  )
}

/** The selectable card every step uses: a calm surface that lifts on hover and shows the focus ring and the choice. */
export const CARD =
  'rounded-xl border border-border-subtle bg-layer-2 text-left transition-[border-color,background-color,box-shadow] duration-150 hover:border-border-strong hover:bg-layer-3 focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none motion-reduce:transition-none'
