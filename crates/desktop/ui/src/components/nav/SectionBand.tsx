// 97b: the tabs ARE the title bar. Every page of a grouped section (Agents · Tasks · Routines, or the seven Library
// pages, or a Settings category) starts with this 52 px band: the sidebar toggle + search when the rail is closed or on
// a phone, then the section's tabs filling the width. No TabsContent: the page below IS the content; a tab is a
// navigation to its route. A popped window has no sidebar and its own OS title bar, so it renders nothing.

import { useNavigate, useRouterState } from '@tanstack/react-router'
import { type LucideIcon, PanelLeft } from 'lucide-react'
import { SearchTrigger } from '@/components/sidebar/SearchTrigger'
import { Button } from '@/components/ui/button'
import { useSidebar } from '@/components/ui/sidebar'
import { Tabs, TabsList, TabsTrigger } from '@/components/ui/tabs'
import { isMacDesktop } from '@/lib/hotkeys'
import { GROUPS, type Group, type Tab, tabOf } from '@/lib/nav'
import { isPopout } from '@/lib/popout'
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
    <TabBand
      tabs={GROUPS[group].filter(tab => supports(discovery, tab.required)).map(({ to, label, icon }) => ({ id: to, label, icon }))}
      value={tabOf(group, pathname) ?? ''}
      onValueChange={(to) => void navigate({ to: to as Tab })}
    />
  )
}

/**
 * The band itself, for any set of tabs (188: a Settings category). `inset: false` where the band does not start at the
 * window's left edge (beside Settings' categories column), so the macOS traffic lights are not its to clear.
 */
export function TabBand({
  tabs,
  value,
  onValueChange,
  inset = true,
}: {
  tabs: readonly BandTab[]
  value: string
  onValueChange: (id: string) => void
  inset?: boolean
}) {
  const sidebar = useSidebar()
  const mac = isMacDesktop()
  if (isPopout()) return null
  return (
    <header
      data-tauri-drag-region={mac || undefined}
      className={cn('flex h-[52px] shrink-0 items-center gap-1 border-b border-border-subtle px-3 max-lg:h-auto max-lg:min-h-[52px]', inset && !sidebar.open && mac && 'pl-20')}
    >
      <SidebarLead />
      <Tabs value={value} onValueChange={onValueChange} className="h-full min-w-0 flex-1 gap-0 max-lg:h-auto">
        <TabsList variant="line" className="w-full gap-0 rounded-none bg-transparent p-0 group-data-[orientation=horizontal]/tabs:h-full max-lg:flex-wrap max-lg:group-data-[orientation=horizontal]/tabs:h-auto">
          {tabs.map(({ id, label, icon: Icon }) => (
            <TabsTrigger
              key={id}
              value={id}
              title={label}
              aria-label={label}
              className="h-full flex-1 rounded-none text-fg-tertiary hover:text-foreground data-[state=active]:text-foreground dark:text-fg-tertiary dark:hover:text-foreground group-data-[orientation=horizontal]/tabs:after:bottom-0 max-lg:h-[52px] max-lg:min-w-10 max-lg:flex-auto"
            >
              <Icon />
              <span className="hidden md:inline">{label}</span>
            </TabsTrigger>
          ))}
        </TabsList>
      </Tabs>
    </header>
  )
}

/** Search and the sidebar toggle, at the left of a page's top bar while the rail is closed or a sheet (phone). */
export function SidebarLead() {
  const sidebar = useSidebar()
  if (sidebar.open && !sidebar.isMobile) return null
  return (
    <div className="flex shrink-0 items-center">
      <SearchTrigger />
      <Button variant="ghost" size="icon-sm" aria-label="Show sidebar" onClick={sidebar.toggleSidebar}>
        <PanelLeft />
      </Button>
    </div>
  )
}
