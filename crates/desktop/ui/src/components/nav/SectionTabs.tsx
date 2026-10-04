// 97 D2: the route-backed tab row under a grouped page's header (Agents · Tasks · Routines, or the seven
// Library pages). No TabsContent: the page below IS the content; a tab is a navigation to its route.

import { useNavigate, useRouterState } from '@tanstack/react-router'
import { Tabs, TabsList, TabsTrigger } from '@/components/ui/tabs'
import { GROUPS, type Group, type Tab, tabOf } from '@/lib/nav'

export function SectionTabs({ group }: { group: Group }) {
  const navigate = useNavigate()
  const pathname = useRouterState({ select: (s) => s.location.pathname })
  // Container query, not viewport: in a 300px rail or on a phone the labels fold away and the tabs are icon-only
  // (title + aria-label carry the name); on a page column they read icon + label. The seven library tabs need 40rem.
  const label = group === 'agents' ? 'hidden @min-[20rem]:inline' : 'hidden @min-[40rem]:inline'
  return (
    <Tabs value={tabOf(group, pathname) ?? ''} onValueChange={(to) => void navigate({ to: to as Tab })} className="@container shrink-0 gap-0">
      <TabsList variant="line" className="w-full justify-start overflow-x-auto border-b border-border-subtle px-2">
        {GROUPS[group].map(({ to, label: name, icon: Icon }) => (
          <TabsTrigger key={to} value={to} title={name} aria-label={name} className="flex-none">
            <Icon />
            <span className={label}>{name}</span>
          </TabsTrigger>
        ))}
      </TabsList>
    </Tabs>
  )
}
