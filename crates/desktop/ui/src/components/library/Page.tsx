import { Link } from '@tanstack/react-router'
import { ChevronLeft, PanelLeft } from 'lucide-react'
import type { ReactNode } from 'react'
import { SectionTabs } from '@/components/nav/SectionTabs'
import { SearchTrigger } from '@/components/sidebar/SearchTrigger'
import { Button } from '@/components/ui/button'
import { useSidebar } from '@/components/ui/sidebar'
import { isMacDesktop } from '@/lib/hotkeys'
import type { Group } from '@/lib/nav'
import { cn } from '@/lib/utils'

/**
 * The Library pages' frame: the 52px header band every sidebar page has (sidebar toggle when it is
 * collapsed, an optional phone-only back link, title, actions) over whatever `children` lay out.
 */
export function Page({
  title,
  back,
  tabs,
  actions,
  children,
}: {
  title: string
  /** Phone-only back link (`/skills` from a skill), shown left of the title. */
  back?: { to: '/skills'; label: string }
  /** 97: the Library tab row under the header, on the group's list pages only (not the skill detail). */
  tabs?: Group
  actions?: ReactNode
  children: ReactNode
}) {
  const sidebar = useSidebar()
  const mac = isMacDesktop()
  return (
    <article className="flex min-h-0 min-w-0 flex-1 flex-col">
      <header
        data-tauri-drag-region={mac || undefined}
        className={cn('flex h-[52px] shrink-0 items-center gap-1 border-b border-border-subtle px-3', !sidebar.open && mac && 'pl-20')}
      >
        {(!sidebar.open || sidebar.isMobile) && (
          <div className="flex shrink-0 items-center">
            <SearchTrigger />
            <Button variant="ghost" size="icon-sm" aria-label="Show sidebar" onClick={sidebar.toggleSidebar}>
              <PanelLeft />
            </Button>
          </div>
        )}
        {back && (
          <Button asChild variant="ghost" size="icon-sm" className="md:hidden" aria-label={back.label}>
            <Link to={back.to}>
              <ChevronLeft />
            </Link>
          </Button>
        )}
        <h2 className="min-w-0 flex-1 truncate text-sm font-semibold">{title}</h2>
        {actions && <div className="flex shrink-0 items-center gap-1">{actions}</div>}
      </header>
      {tabs && <SectionTabs group={tabs} />}
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
