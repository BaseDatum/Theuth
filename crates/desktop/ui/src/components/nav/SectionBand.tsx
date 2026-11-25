// 97b: the tabs ARE the title bar. Every page of a grouped section (Agents · Tasks · Routines, or the seven Library
// pages, or a Settings category) starts with this 52 px band: the sidebar toggle + search when the rail is closed or on
// a phone, then the section's tabs filling the width. No TabsContent: the page below IS the content; a tab is a
// navigation to its route. A popped window has no sidebar and its own OS title bar, so it renders nothing — except a
// page window (`page` popout), whose tabs are how it moves between the section's pages.

import { useNavigate, useRouterState } from '@tanstack/react-router'
import { type LucideIcon, PanelLeft } from 'lucide-react'
import { useViewDrag } from '@/components/chat/Panes'
import { SearchTrigger } from '@/components/sidebar/SearchTrigger'
import { Button } from '@/components/ui/button'
import { useSidebar } from '@/components/ui/sidebar'
import { cornerPad, headerToggle } from '@/lib/sidebar-corner'
import { Tabs, TabsList, TabsTrigger } from '@/components/ui/tabs'
import { isMacDesktop } from '@/lib/hotkeys'
import { LibraryOverview } from '@/components/library/Overview'
import { GROUPS, type Group, type Tab, tabOf } from '@/lib/nav'
import { POPOUT, isPopout } from '@/lib/popout'
import { useInsideTile } from '@/components/tiles/tile-context'
import { cn } from '@/lib/utils'
import { supports } from '@/rpc/discovery'
import { useConnection } from '@/store/connection'

/** One tab of a band: `id` is what `onValueChange` receives. */
export type BandTab = { id: string; label: string; icon: LucideIcon }

/** The band of a sidebar group (Agents, Library), whose tabs are its routes. */
export function SectionBand({ group }: { group: Group }) {
  const discovery = useConnection(s => s.discovery)
  const navigate = useNavigate()
  const pathname = useRouterState({ select: (s) => s.location.pathname })
  return (
    <>
      <TabBand
        tabs={GROUPS[group].filter(tab => supports(discovery, tab.required)).map(({ to, label, icon }) => ({ id: to, label, icon }))}
        value={tabOf(group, pathname) ?? ''}
        onValueChange={(to) => void navigate({ to: to as Tab })}
        tour={group === 'library' ? 'library-tabs' : undefined}
        draggable
      />
      {/* 215: what each Library tab holds, under the band. */}
      {group === 'library' && <LibraryOverview />}
    </>
  )
}

/**
 * The band itself, for any set of tabs (188: a Settings category). `inset: false` where the band does not start at the
 * window's left edge (beside Settings' categories column), so the macOS traffic lights are not its to clear.
 */
function DraggableTabTrigger({ id, label, icon: Icon, insideTile }: BandTab & { insideTile: boolean }) {
  const drag = useViewDrag(id, label)
  return (
    <TabsTrigger
      value={id}
      title={label}
      aria-label={label}
      {...drag}
      className={cn(
        'h-full flex-1 rounded-none text-fg-tertiary hover:text-foreground data-[state=active]:text-foreground dark:text-fg-tertiary dark:hover:text-foreground group-data-[orientation=horizontal]/tabs:after:bottom-0 max-lg:min-w-10 max-lg:flex-auto',
        insideTile ? 'max-lg:h-10' : 'max-lg:h-[52px]',
      )}
    >
      <Icon />
      <span className="hidden md:inline">{label}</span>
    </TabsTrigger>
  )
}

export function TabBand({
  tabs,
  value,
  onValueChange,
  inset = true,
  tour,
  draggable = false,
}: {
  tabs: readonly BandTab[]
  value: string
  onValueChange: (id: string) => void
  inset?: boolean
  /** 215: the band's `data-tour` anchor. */
  tour?: string
  /** When true, each tab's `id` is a route path that can be dragged into a pane (`useViewDrag`). */
  draggable?: boolean
}) {
  const sidebar = useSidebar()
  const mac = isMacDesktop()
  const insideTile = useInsideTile()
  if (!insideTile && isPopout() && POPOUT?.kind !== 'page') return null
  return (
    <header
      data-tauri-drag-region={(!insideTile && mac) || undefined}
      data-tour={tour}
      className={cn(
        'flex shrink-0 items-center gap-1 border-b border-border-subtle px-3 max-lg:h-auto',
        insideTile ? 'h-10 max-lg:min-h-10' : 'h-[52px] max-lg:min-h-[52px]',
        !insideTile && inset && mac && cornerPad(sidebar),
      )}
    >
      {!insideTile && <SidebarLead />}
      <Tabs value={value} onValueChange={onValueChange} className="h-full min-w-0 flex-1 gap-0 max-lg:h-auto">
        <TabsList variant="line" className="w-full gap-0 rounded-none bg-transparent p-0 group-data-[orientation=horizontal]/tabs:h-full max-lg:flex-wrap max-lg:group-data-[orientation=horizontal]/tabs:h-auto">
          {tabs.map((tab) =>
            draggable ? (
              <DraggableTabTrigger key={tab.id} {...tab} insideTile={insideTile} />
            ) : (
              <TabsTrigger
                key={tab.id}
                value={tab.id}
                title={tab.label}
                aria-label={tab.label}
                className={cn(
                  'h-full flex-1 rounded-none text-fg-tertiary hover:text-foreground data-[state=active]:text-foreground dark:text-fg-tertiary dark:hover:text-foreground group-data-[orientation=horizontal]/tabs:after:bottom-0 max-lg:min-w-10 max-lg:flex-auto',
                  insideTile ? 'max-lg:h-10' : 'max-lg:h-[52px]',
                )}
              >
                <tab.icon />
                <span className="hidden md:inline">{tab.label}</span>
              </TabsTrigger>
            ),
          )}
        </TabsList>
      </Tabs>
    </header>
  )
}

/** Search and the sidebar toggle, at the left of a page's top bar while the rail is closed or a sheet (phone); never in a popped window, which has no rail. */
export function SidebarLead() {
  const sidebar = useSidebar()
  const insideTile = useInsideTile()
  if (insideTile || isPopout() || !headerToggle(sidebar)) return null
  return (
    <div className="flex shrink-0 items-center">
      <SearchTrigger />
      <Button variant="ghost" size="icon-sm" aria-label="Show sidebar" onClick={sidebar.toggleSidebar}>
        <PanelLeft />
      </Button>
    </div>
  )
}
