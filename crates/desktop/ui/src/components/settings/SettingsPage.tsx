import './small-screens.css'
import { useQuery } from '@tanstack/react-query'
import { Link, useNavigate } from '@tanstack/react-router'
import { ChevronRight } from 'lucide-react'
import { SidebarLead, TabBand, type BandTab } from '@/components/nav/SectionBand'
import { useInsideTile } from '@/components/tiles/tile-context'
import { SectionRow } from '@/components/nav/SectionRow'
import { RestartBanner } from '@/components/settings/page/RestartBanner'
import { bandTabs, deviceTabs, isDeviceTab, navCategories, navTabs, placeOf } from '@/components/settings/registry'
import { NotHereTag, ScopeHeader, ScopeTag } from '@/components/settings/ScopeTag'
import { type EngineCaps, type ScopeNames, type SettingsNeed, deviceKind, engineCaps, notApplicable, sharedScope } from '@/components/settings/scope'
import type { SettingsCategory, SettingsTab } from '@/components/settings/types'
import { keys as voiceKeys } from '@/components/settings/Voice'
import { useMesh } from '@/components/tasks/queries'
import { useSidebar } from '@/components/ui/sidebar'
import { cornerPad } from '@/lib/sidebar-corner'
import { useIsMobile } from '@/hooks/use-mobile'
import { isMacDesktop } from '@/lib/hotkeys'
import { isDesktop } from '@/lib/platform'
import { useApi } from '@/lib/rpc'
import { cn } from '@/lib/utils'
import { LOCAL, useEngines } from '@/link/engines'
import { Capability, useQuerySupport } from '@/rpc/capabilities'
import { useConnection } from '@/store/connection'

/** Category links keep a visible focus ring, as the band's tabs do. */
const FOCUS_RING = 'outline-none focus-visible:ring-[3px] focus-visible:ring-ring/50'

/** The tooltips' names: the connected engine (`info.name`), the hub (`mesh_status`'s lease holder), this client's kind. */
function useScopeNames(): ScopeNames {
  const engine = useConnection((s) => s.info?.name ?? null)
  const mesh = useMesh().data
  const holder = mesh?.lease.holder ?? null
  const hub = !mesh || holder === null ? null : holder === mesh.self_id ? mesh.self_name : (mesh.peers.find((p) => p.id === holder)?.name ?? null)
  return { engine, hub, device: deviceKind() }
}

/** What the connected engine's host has (`engineCaps`): the desktop's own daemon has everything; elsewhere the mic list tells. */
function useEngineCaps(): EngineCaps {
  const local = useEngines((s) => s.active) === LOCAL && isDesktop()
  const canList = useQuerySupport(['list_input_devices'])
  const api = useApi()
  // Same key as Voice › Microphone, so opening it after the nav asks nothing twice.
  const devices = useQuery({ enabled: canList && !local, queryKey: voiceKeys.devices, queryFn: () => api.list_input_devices() })
  return engineCaps({ local, inputDevices: devices.data })
}

/** The needs every one of `tabs` lacks here, or `null` when any of them applies. */
function allMissing(tabs: readonly SettingsTab[], caps: EngineCaps): SettingsNeed[] | null {
  const missing = tabs.map((tab) => notApplicable(tab, caps))
  if (missing.length === 0 || missing.some((m) => m === null)) return null
  return [...new Set(missing.flatMap((m) => m ?? []))]
}

/** A band tab with its mark: `n/a` (dimmed, still opens) when the engine lacks a need, else the tag of a shared scope. */
function bandTab(tab: SettingsTab, caps: EngineCaps, names: ScopeNames): BandTab {
  const missing = notApplicable(tab, caps)
  if (missing) return { id: tab.id, label: tab.label, icon: tab.icon, dim: true, hint: `${tab.label}: not on this machine`, tag: <NotHereTag missing={missing} engine={names.engine} /> }
  // `machine` is the default and `device` is the band's own group: only the scopes that differ get a tag.
  const tag = tab.scope === 'everywhere' || tab.scope === 'hub' ? <ScopeTag scope={tab.scope} names={names} /> : undefined
  return { id: tab.id, label: tab.label, icon: tab.icon, tag }
}

/** A category row's mark: `n/a` when none of its tabs applies here, else the scope all its tabs share (not `machine`). */
function CategoryMark({ category, caps, names }: { category: SettingsCategory; caps: EngineCaps; names: ScopeNames }) {
  const discovery = useConnection((s) => s.discovery)
  const tabs = navTabs(category, discovery)
  const missing = allMissing(tabs, caps)
  if (missing) return <NotHereTag missing={missing} engine={names.engine} className="ml-auto" />
  const scope = sharedScope(tabs)
  return scope && scope !== 'machine' ? <ScopeTag scope={scope} names={names} className="ml-auto" /> : null
}

/**
 * 188 S1–S3: Settings as a page in the main pane. Desktop and iPad: the categories column, then the category's tabs in
 * the section band (the tabs are the title, 97b) over the tab's body. Phone: the band with icon tabs, a back row to the
 * category list (`/settings`), then the body. You leave by navigating or with Back: no close button, no Esc.
 * Per-machine Settings: device tabs leave their category for a "This <device>" group; every tab is tagged with its scope.
 */
export function SettingsPage({ tab, focus }: { tab: string; focus: string | null }) {
  const discovery = useConnection(s => s.discovery)
  const mobile = useIsMobile()
  const navigate = useNavigate()
  const names = useScopeNames()
  const caps = useEngineCaps()
  // `beforeLoad` sent unknown and hidden ids elsewhere.
  const placed = placeOf(tab)
  if (!placed) return null
  const { category, tab: current } = placed
  const band = (
    <TabBand
      tabs={bandTabs(placed, discovery).map((t) => bandTab(t, caps, names))}
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
        <ScopeHeader scope={current.scope} names={names} missing={notApplicable(current, caps)} />
        {current.required ? <Capability required={current.required}><current.Component focus={focus} /></Capability> : <current.Component focus={focus} />}
      </div>
    </div>
  )
  if (mobile) {
    return (
      <article data-settings-page="" className="flex min-h-0 min-w-0 flex-1 flex-col">
        {band}
        <SectionRow back={{ to: '/settings', label: 'Back to settings' }} title={isDeviceTab(current) ? `This ${names.device}` : category.label} />
        {body}
      </article>
    )
  }
  return (
    <article data-settings-page="" className="flex min-h-0 min-w-0 flex-1">
      <Categories current={category} currentTab={current} caps={caps} names={names} />
      <div className="flex min-w-0 flex-1 flex-col">
        {band}
        {body}
      </div>
    </article>
  )
}

/** The "This <device>" group's heading, as the mock's section labels. */
function DeviceHeading({ names, className }: { names: ScopeNames; className?: string }) {
  return <div className={cn('px-2.5 pt-3 pb-1 text-[11px] tracking-wide text-fg-tertiary uppercase', className)}>This {names.device}</div>
}

/** The categories column; its 52 px head lines up with the band and is the column's label, not a title bar. */
function Categories({ current, currentTab, caps, names }: { current: SettingsCategory; currentTab: SettingsTab; caps: EngineCaps; names: ScopeNames }) {
  const discovery = useConnection(s => s.discovery)
  const sidebar = useSidebar()
  const mac = isMacDesktop()
  const insideTile = useInsideTile()
  const device = deviceTabs(discovery)
  return (
    <nav aria-label="Settings categories" className="flex w-[208px] shrink-0 flex-col border-r border-border-subtle">
      {/* With the rail closed this column meets the window's left edge, so it clears the traffic lights, not the band. */}
      <div
        data-tauri-drag-region={(!insideTile && mac) || undefined}
        className={cn(
          'flex shrink-0 items-center border-b border-border-subtle px-5 text-sm font-semibold',
          insideTile ? 'h-10' : 'h-[52px]',
          !insideTile && mac && cornerPad(sidebar),
        )}
      >
        Settings
      </div>
      <div className="flex min-h-0 flex-1 flex-col gap-0.5 overflow-y-auto p-2">
        {navCategories(discovery).map((category) => {
          const Icon = category.icon
          const on = category === current && !isDeviceTab(currentTab)
          return (
            <Link
              key={category.id}
              to="/settings/$tab"
              params={{ tab: navTabs(category, discovery)[0]!.id }}
              aria-current={on ? 'page' : undefined}
              className={cn('flex h-9 shrink-0 items-center gap-2.5 rounded-[10px] px-2.5 text-sm', FOCUS_RING, on ? 'bg-layer-3' : 'hover:bg-layer-3/60')}
            >
              <Icon className="size-4 text-fg-secondary" />
              <span className="min-w-0 truncate">{category.label}</span>
              <CategoryMark category={category} caps={caps} names={names} />
            </Link>
          )
        })}
        {device.length > 0 && <DeviceHeading names={names} />}
        {device.map((tab) => {
          const on = tab === currentTab
          return (
            <Link
              key={tab.id}
              to="/settings/$tab"
              params={{ tab: tab.id }}
              aria-current={on ? 'page' : undefined}
              className={cn('flex h-9 shrink-0 items-center gap-2.5 rounded-[10px] px-2.5 text-sm', FOCUS_RING, on ? 'bg-layer-3' : 'hover:bg-layer-3/60')}
            >
              <tab.icon className="size-4 text-fg-secondary" />
              <span className="min-w-0 truncate">{tab.label}</span>
              <ScopeTag scope={tab.scope} names={names} className="ml-auto" />
            </Link>
          )
        })}
      </div>
    </nav>
  )
}

/** 188 S3: `/settings` on a phone: one row per category, opening its first shown tab, then the "This <device>" group. */
export function SettingsHome() {
  const discovery = useConnection(s => s.discovery)
  const insideTile = useInsideTile()
  const names = useScopeNames()
  const caps = useEngineCaps()
  const device = deviceTabs(discovery)
  const row = cn('flex h-12 items-center gap-3 rounded-[10px] px-3 text-base active:bg-layer-3', FOCUS_RING)
  return (
    <article data-settings-page="" className="flex min-h-0 min-w-0 flex-1 flex-col">
      <header className={cn('flex shrink-0 items-center gap-1 border-b border-border-subtle px-3', insideTile ? 'h-10' : 'h-[52px]')}>
        {!insideTile && <SidebarLead />}
        <h2 className="px-1 text-sm font-semibold">Settings</h2>
      </header>
      <nav aria-label="Settings categories" className="min-h-0 flex-1 overflow-y-auto p-2">
        {navCategories(discovery).map((category) => (
          <Link key={category.id} to="/settings/$tab" params={{ tab: navTabs(category, discovery)[0]!.id }} className={row}>
            <category.icon className="size-5 text-fg-secondary" />
            <span className="flex-1">{category.label}</span>
            <CategoryMark category={category} caps={caps} names={names} />
            <ChevronRight className="size-4 text-fg-tertiary" />
          </Link>
        ))}
        {device.length > 0 && <DeviceHeading names={names} className="px-3" />}
        {device.map((tab) => (
          <Link key={tab.id} to="/settings/$tab" params={{ tab: tab.id }} className={row}>
            <tab.icon className="size-5 text-fg-secondary" />
            <span className="flex-1">{tab.label}</span>
            <ScopeTag scope={tab.scope} names={names} />
            <ChevronRight className="size-4 text-fg-tertiary" />
          </Link>
        ))}
      </nav>
    </article>
  )
}
