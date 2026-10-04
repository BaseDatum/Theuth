// 97b: the 44 px row under the SectionBand that carries a page's own things: back chevron, title (detail pages),
// sub-tabs, filters, buttons. No drag region, no traffic-light inset, no sidebar toggle: those are the band's.

import { Link } from '@tanstack/react-router'
import { ChevronLeft } from 'lucide-react'
import type { ReactNode } from 'react'
import { Button } from '@/components/ui/button'
import type { Tab } from '@/lib/nav'
import { cn } from '@/lib/utils'

export function SectionRow({
  back,
  title,
  children,
  className,
}: {
  /** A link back to the section's list; `className: 'md:hidden'` where the back is phone-only. */
  back?: { to: Tab; label: string; className?: string }
  /** A detail page's title; when absent, put `ml-auto` on the first right-aligned control. */
  title?: ReactNode
  children?: ReactNode
  className?: string
}) {
  return (
    <div className={cn('flex h-11 shrink-0 items-center gap-1 border-b border-border-subtle px-3', className)}>
      {back && (
        <Button asChild variant="ghost" size="icon-sm" className={back.className} aria-label={back.label}>
          <Link to={back.to}>
            <ChevronLeft />
          </Link>
        </Button>
      )}
      {title !== undefined && <h2 className="min-w-0 flex-1 truncate text-sm font-semibold">{title}</h2>}
      {children}
    </div>
  )
}
