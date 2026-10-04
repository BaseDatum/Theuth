// 97b: the tabs ARE the title bar. Every page of a grouped section (Agents · Tasks · Routines, or the seven Library
// pages) starts with this 52 px band: the sidebar toggle + search when the rail is closed or on a phone, then the
// section's tabs filling the width. No TabsContent: the page below IS the content; a tab is a navigation to its route.
// A popped window has no sidebar and its own OS title bar, so it renders nothing.

import { useNavigate, useRouterState } from '@tanstack/react-router'
import { PanelLeft } from 'lucide-react'
import { SearchTrigger } from '@/components/sidebar/SearchTrigger'
import { Button } from '@/components/ui/button'
import { useSidebar } from '@/components/ui/sidebar'
import { Tabs, TabsList, TabsTrigger } from '@/components/ui/tabs'
import { isMacDesktop } from '@/lib/hotkeys'
import { GROUPS, type Group, type Tab, tabOf } from '@/lib/nav'
import { isPopout } from '@/lib/popout'
import { cn } from '@/lib/utils'

export function SectionBand({ group }: { group: Group }) {
  const sidebar = useSidebar()
  const mac = isMacDesktop()
  const navigate = useNavigate()
  const pathname = useRouterState({ select: (s) => s.location.pathname })
  if (isPopout()) return null
  return (
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
      <Tabs value={tabOf(group, pathname) ?? ''} onValueChange={(to) => void navigate({ to: to as Tab })} className="h-full min-w-0 flex-1 gap-0">
        <TabsList variant="line" className="w-full gap-0 rounded-none bg-transparent p-0 group-data-[orientation=horizontal]/tabs:h-full">
          {GROUPS[group].map(({ to, label, icon: Icon }) => (
            <TabsTrigger
              key={to}
              value={to}
              title={label}
              aria-label={label}
              className="h-full flex-1 rounded-none text-fg-tertiary hover:text-foreground data-[state=active]:text-foreground dark:text-fg-tertiary dark:hover:text-foreground group-data-[orientation=horizontal]/tabs:after:bottom-0"
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
