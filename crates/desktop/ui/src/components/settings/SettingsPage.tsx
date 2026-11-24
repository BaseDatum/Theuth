import './small-screens.css'
import { Link, useNavigate } from '@tanstack/react-router'
import { ChevronRight } from 'lucide-react'
import { SidebarLead, TabBand } from '@/components/nav/SectionBand'
import { SectionRow } from '@/components/nav/SectionRow'
import { RestartBanner } from '@/components/settings/page/RestartBanner'
import { placeOf, shownCategories, shownTab, shownTabs } from '@/components/settings/registry'
import type { SettingsCategory } from '@/components/settings/types'
import { useSidebar } from '@/components/ui/sidebar'
import { cornerPad } from '@/lib/sidebar-corner'
import { useIsMobile } from '@/hooks/use-mobile'
import { isMacDesktop } from '@/lib/hotkeys'
import { cn } from '@/lib/utils'
import { Capability } from '@/rpc/capabilities'
import { useConnection } from '@/store/connection'

/** Category links keep a visible focus ring, as the band's tabs do. */
const FOCUS_RING = 'outline-none focus-visible:ring-[3px] focus-visible:ring-ring/50'

/**
 * 188 S1–S3: Settings as a page in the main pane. Desktop and iPad: the categories column, then the category's tabs in
 * the section band (the tabs are the title, 97b) over the tab's body. Phone: the band with icon tabs, a back row to the
 * category list (`/settings`), then the body. You leave by navigating or with Back: no close button, no Esc.
 */
export function SettingsPage({ tab, focus }: { tab: string; focus: string | null }) {
  const discovery = useConnection(s => s.discovery)
  const mobile = useIsMobile()
  const navigate = useNavigate()
  // `beforeLoad` sent unknown and hidden ids elsewhere.
  const placed = placeOf(tab)
  if (!placed) return null
  const { category, tab: current } = placed
  const band = (
    <TabBand
      tabs={shownTabs(category, discovery)}
      value={tab}
      inset={mobile}
      onValueChange={(id) => void navigate({ to: '/settings/$tab', params: { tab: id } })}
    />
  )
  // Keyed by tab: a new tab starts at the top.
  const body = (
    <div key={tab} className="min-h-0 min-w-0 flex-1 overflow-y-auto max-lg:@container/settings">
      <div className="mx-auto w-full max-w-[720px] px-4 pt-4 pb-8 md:px-6">
        <RestartBanner />
        {current.required ? <Capability required={current.required}><current.Component focus={focus} /></Capability> : <current.Component focus={focus} />}
      </div>
    </div>
  )
  if (mobile) {
    return (
      <article data-settings-page="" className="flex min-h-0 min-w-0 flex-1 flex-col">
        {band}
        <SectionRow back={{ to: '/settings', label: 'Back to settings' }} title={category.label} />
        {body}
      </article>
    )
  }
  return (
    <article data-settings-page="" className="flex min-h-0 min-w-0 flex-1">
      <Categories current={category} />
      <div className="flex min-w-0 flex-1 flex-col">
        {band}
        {body}
      </div>
    </article>
  )
}

/** The categories column; its 52 px head lines up with the band and is the column's label, not a title bar. */
function Categories({ current }: { current: SettingsCategory }) {
  const discovery = useConnection(s => s.discovery)
  const sidebar = useSidebar()
  const mac = isMacDesktop()
  return (
    <nav aria-label="Settings categories" className="flex w-[208px] shrink-0 flex-col border-r border-border-subtle">
      {/* With the rail closed this column meets the window's left edge, so it clears the traffic lights, not the band. */}
      <div
        data-tauri-drag-region={mac || undefined}
        className={cn('flex h-[52px] shrink-0 items-center border-b border-border-subtle px-5 text-sm font-semibold', mac && cornerPad(sidebar))}
      >
        Settings
      </div>
      <div className="flex min-h-0 flex-1 flex-col gap-0.5 overflow-y-auto p-2">
        {shownCategories(discovery).map((category) => {
          const Icon = category.icon
          const on = category === current
          return (
            <Link
              key={category.id}
              to="/settings/$tab"
              params={{ tab: shownTab(category.tabs[0].id, discovery) }}
              aria-current={on ? 'page' : undefined}
              className={cn('flex h-9 shrink-0 items-center gap-2.5 rounded-[10px] px-2.5 text-sm', FOCUS_RING, on ? 'bg-layer-3' : 'hover:bg-layer-3/60')}
            >
              <Icon className="size-4 text-fg-secondary" />
              {category.label}
            </Link>
          )
        })}
      </div>
    </nav>
  )
}

/** 188 S3: `/settings` on a phone: one row per category, opening its first shown tab. */
export function SettingsHome() {
  const discovery = useConnection(s => s.discovery)
  return (
    <article data-settings-page="" className="flex min-h-0 min-w-0 flex-1 flex-col">
      <header className="flex h-[52px] shrink-0 items-center gap-1 border-b border-border-subtle px-3">
        <SidebarLead />
        <h2 className="px-1 text-sm font-semibold">Settings</h2>
      </header>
      <nav aria-label="Settings categories" className="min-h-0 flex-1 overflow-y-auto p-2">
        {shownCategories(discovery).map(({ id, label, icon: Icon, tabs }) => (
          <Link
            key={id}
            to="/settings/$tab"
            params={{ tab: shownTab(tabs[0].id, discovery) }}
            className={cn('flex h-12 items-center gap-3 rounded-[10px] px-3 text-base active:bg-layer-3', FOCUS_RING)}
          >
            <Icon className="size-5 text-fg-secondary" />
            <span className="flex-1">{label}</span>
            <ChevronRight className="size-4 text-fg-tertiary" />
          </Link>
        ))}
      </nav>
    </article>
  )
}
