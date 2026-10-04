import type { ReactNode } from 'react'
import { SectionBand } from '@/components/nav/SectionBand'
import { SectionRow } from '@/components/nav/SectionRow'

/**
 * The Library pages' frame (97b): the Library band (the tab is the title), then the page's row — an optional
 * phone-only back link and the right-aligned actions — over whatever `children` lay out.
 */
export function Page({
  back,
  actions,
  children,
}: {
  /** Phone-only back link (`/skills` from a skill). */
  back?: { to: '/skills'; label: string }
  actions?: ReactNode
  children: ReactNode
}) {
  return (
    <article className="flex min-h-0 min-w-0 flex-1 flex-col">
      <SectionBand group="library" />
      <SectionRow back={back && { ...back, className: 'md:hidden' }}>
        {actions && <div className="ml-auto flex shrink-0 items-center gap-1">{actions}</div>}
      </SectionRow>
      {children}
    </article>
  )
}

/** A scrolling, centred reading column for the single-list pages (Tools, Secrets). */
export function Column({ children }: { children: ReactNode }) {
  return (
    <div className="min-h-0 flex-1 overflow-y-auto">
      <div className="mx-auto w-full max-w-3xl px-6 py-4">{children}</div>
    </div>
  )
}
