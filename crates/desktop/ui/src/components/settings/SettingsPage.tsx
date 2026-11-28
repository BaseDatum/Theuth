import './small-screens.css'
import { useQuery } from '@tanstack/react-query'
import { Link, useNavigate } from '@tanstack/react-router'
import { ChevronRight } from 'lucide-react'
import { type ReactNode, useState } from 'react'
import { useStore } from 'zustand'
import { SidebarLead, TabBand, type BandTab } from '@/components/nav/SectionBand'
import { useInsideTile } from '@/components/tiles/tile-context'
import { SectionRow } from '@/components/nav/SectionRow'
import { RestartBanner } from '@/components/settings/page/RestartBanner'
import { SetOnAllSheet, type SetOnAllRequest } from '@/components/settings/copy/SetOnAllSheet'
import { MachinePicker } from '@/components/settings/MachinePicker'
import { MachineScope, type SettingsMachines, SettingsMachinesProvider, isRemote, pickerTargetFor, resolveTabBinding, usePickedMachine, useSettingsMachinesValue } from '@/components/settings/machine-target'
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
import type { Machine } from '@/lib/mesh-machines'
import { isDesktop } from '@/lib/platform'
import { useApi } from '@/lib/rpc'
import { cn } from '@/lib/utils'
import { LOCAL, useEngines } from '@/link/engines'
import { Capability, useQuerySupport } from '@/rpc/capabilities'
import type { Discovery } from '@/rpc/discovery'
import { useConnection } from '@/store/connection'
import { StoresProvider, scope } from '@/store/engine-scope'

/** Category links keep a visible focus ring, as the band's tabs do. */
const FOCUS_RING = 'outline-none focus-visible:ring-[3px] focus-visible:ring-ring/50'

/** The paired hub's discovery when `machines.meshClient` differs from the active engine, else `undefined`. */
function useHubDiscovery(machines: SettingsMachines): Discovery | undefined {
  const active = useEngines((s) => s.active)
  const hubDiscovery = useStore(scope(machines.meshClient).connection, (s) => s.discovery)
  return machines.meshClient !== active ? hubDiscovery : undefined
}

/**
 * The tooltips' names: the machine Settings edits (227: the picked one, else the connected engine's `info.name`), the
 * hub (`mesh_status`'s lease holder), this client's kind.
 */
function useScopeNames(machines: SettingsMachines): ScopeNames {
  const connected = useConnection((s) => s.info?.name ?? null)
  const mesh = useMesh().data
  const holder = mesh?.lease.holder ?? null
  const hub =
    (!mesh || holder === null ? null : holder === mesh.self_id ? mesh.self_name : (mesh.peers.find((p) => p.id === holder)?.name ?? null)) ??
    machines.machines.find((m) => m.hub)?.name ??
    null
  return { engine: isRemote(machines.target) ? machines.target.name : connected, hub, device: deviceKind() }
}

/**
 * What the edited machine's host has (`engineCaps`): its `engine_caps` when known (227); else, for the connected
 * engine, the desktop's own daemon has everything and elsewhere the mic list tells.
 */
function useEngineCaps(machines: SettingsMachines): EngineCaps {
  const local = useEngines((s) => s.active) === LOCAL && isDesktop()
  const canList = useQuerySupport(['list_input_devices'])
  const api = useApi()
  const target = machines.target
  // Same key as Voice › Microphone, so opening it after the nav asks nothing twice.
  const devices = useQuery({ enabled: canList && !local && !target?.caps, queryKey: voiceKeys.devices, queryFn: () => api.list_input_devices() })
  if (target?.caps) return engineCaps({ local: false, host: target.caps })
  if (isRemote(target)) return {}
  return engineCaps({ local, inputDevices: devices.data })
}

/** The page's machines (227) and the "Set on all machines…" sheet around `children`. */
function WithMachines({ tab, children }: { tab: SettingsTab | null; children: (machines: SettingsMachines) => ReactNode }) {
  const [request, setRequest] = useState<SetOnAllRequest | null>(null)
  const machines = useSettingsMachinesValue(tab, setRequest)
  return (
    <SettingsMachinesProvider value={machines}>
      {children(machines)}
      <SetOnAllSheet request={request} settings={machines} onClose={() => setRequest(null)} />
    </SettingsMachinesProvider>
  )
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
function CategoryMark({ category, caps, names, hubDiscovery }: { category: SettingsCategory; caps: EngineCaps; names: ScopeNames; hubDiscovery?: Discovery }) {
  const discovery = useConnection((s) => s.discovery)
  const tabs = navTabs(category, discovery, hubDiscovery)
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
export function SettingsPage({ tab, focus, progress = null }: { tab: string; focus: string | null; progress?: string | null }) {
  // `beforeLoad` sent unknown and hidden ids elsewhere.
  const placed = placeOf(tab)
  if (!placed) return null
  return <WithMachines tab={placed.tab}>{(machines) => <Page tab={tab} focus={focus} progress={progress} machines={machines} />}</WithMachines>
}

function Page({ tab, focus, progress, machines }: { tab: string; focus: string | null; progress: string | null; machines: SettingsMachines }) {
  const discovery = useConnection((s) => s.discovery)
  const active = useEngines((s) => s.active)
  const hubDiscovery = useHubDiscovery(machines)
  const picked = usePickedMachine((s) => s.picked)
  const mobile = useIsMobile()
  const navigate = useNavigate()
  const names = useScopeNames(machines)
  const caps = useEngineCaps(machines)
  const placed = placeOf(tab)!
  const { category, tab: current } = placed
  const { target } = machines
  const binding = resolveTabBinding({
    categoryId: category.id,
    tab: current,
    target,
    picked,
    machines: machines.machines,
    active,
    meshClient: machines.meshClient,
    discovery,
    hubDiscovery,
  })
  const { aimed, scopeClient, loading } = binding
  const pickerTarget = pickerTargetFor(binding, picked, machines.machines, target)
  const tabNames = aimed && aimed !== target ? { ...names, engine: aimed.name } : names
  const band = (
    <TabBand
      tabs={bandTabs(placed, discovery, hubDiscovery).map((t) => bandTab(t, caps, names))}
      value={tab}
      inset={mobile}
      onValueChange={(id) => void navigate({ to: '/settings/$tab', params: { tab: id } })}
    />
  )
  const content = loading ? (
    <p role="status" className="py-4 text-sm text-fg-tertiary">Loading mesh…</p>
  ) : current.required ? (
    <Capability required={current.required}><current.Component focus={focus} progress={progress} /></Capability>
  ) : (
    <current.Component focus={focus} progress={progress} />
  )
  // Keyed by tab and, for a machine tab, the machine: a new tab or machine starts at the top with its own state.
  const body = (
    <div key={`${tab}@${aimed?.id ?? scopeClient ?? ''}`} className="min-h-0 min-w-0 flex-1 overflow-y-auto max-lg:@container/settings">
      <div className="mx-auto w-full max-w-[720px] px-4 pt-4 pb-8 md:px-6">
        <RestartBanner />
        <ScopeHeader
          scope={current.scope}
          names={tabNames}
          missing={notApplicable(current, caps)}
          editing={machines.machines.length > 1 ? (aimed ?? machines.connected)?.name : null}
          ignoring={isRemote(target) ? target.name : null}
          connectedOnly={current.scope === 'machine' && isRemote(target) && !aimed ? target.name : null}
        />
        {scopeClient ? (
          <StoresProvider stores={scope(scopeClient)}>{content}</StoresProvider>
        ) : (
          <MachineScope machine={aimed}>{content}</MachineScope>
        )}
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
      <Categories current={category} currentTab={current} caps={caps} names={names} hubDiscovery={hubDiscovery} pickerTarget={pickerTarget} />
      <div className="@container/settings flex min-w-0 flex-1 flex-col">
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
function Categories({
  current,
  currentTab,
  caps,
  names,
  hubDiscovery,
  pickerTarget,
}: {
  current: SettingsCategory
  currentTab: SettingsTab
  caps: EngineCaps
  names: ScopeNames
  hubDiscovery?: Discovery
  pickerTarget?: Machine | null
}) {
  const discovery = useConnection((s) => s.discovery)
  const sidebar = useSidebar()
  const mac = isMacDesktop()
  const insideTile = useInsideTile()
  const device = deviceTabs(discovery)
  return (
    <nav aria-label="Settings categories" className="flex w-[240px] shrink-0 flex-col border-r border-border-subtle">
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
      <MachinePicker selected={pickerTarget} className="mx-2 mt-2 w-auto" />
      <div className="flex min-h-0 flex-1 flex-col gap-0.5 overflow-y-auto p-2">
        {navCategories(discovery, hubDiscovery).map((category) => {
          const Icon = category.icon
          const on = category === current && !isDeviceTab(currentTab)
          return (
            <Link
              key={category.id}
              to="/settings/$tab"
              params={{ tab: navTabs(category, discovery, hubDiscovery)[0]!.id }}
              aria-current={on ? 'page' : undefined}
              className={cn('flex h-9 shrink-0 items-center gap-2.5 rounded-[10px] px-2.5 text-sm', FOCUS_RING, on ? 'bg-layer-3' : 'hover:bg-layer-3/60')}
            >
              <Icon className="size-4 text-fg-secondary" />
              <span className="min-w-0 truncate">{category.label}</span>
              <CategoryMark category={category} caps={caps} names={names} hubDiscovery={hubDiscovery} />
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

/** 188 S3: `/settings` on a phone: one row per category, opening its first shown tab, then the "This <device>" group. 227: the machine picker heads it. */
export function SettingsHome() {
  return <WithMachines tab={null}>{(machines) => <Home machines={machines} />}</WithMachines>
}

function Home({ machines }: { machines: SettingsMachines }) {
  const discovery = useConnection((s) => s.discovery)
  const hubDiscovery = useHubDiscovery(machines)
  const picked = usePickedMachine((s) => s.picked)
  const active = useEngines((s) => s.active)
  // Phone home has no open tab. While the pick is still the default, show the paired hub so a tap on
  // This iPhone is an explicit pick, not an accidental one that would hide Network's hub controls.
  const homeHub =
    picked === null && machines.meshClient !== active
      ? (machines.machines.find((m) => m.client === machines.meshClient && !m.forwarded) ?? null)
      : null
  const insideTile = useInsideTile()
  const names = useScopeNames(machines)
  const caps = useEngineCaps(machines)
  const device = deviceTabs(discovery)
  const row = cn('flex h-12 items-center gap-3 rounded-[10px] px-3 text-base active:bg-layer-3', FOCUS_RING)
  return (
    <article data-settings-page="" className="flex min-h-0 min-w-0 flex-1 flex-col">
      <header className={cn('flex shrink-0 items-center gap-1 border-b border-border-subtle px-3', insideTile ? 'h-10' : 'h-[52px]')}>
        {!insideTile && <SidebarLead />}
        <h2 className="px-1 text-sm font-semibold">Settings</h2>
      </header>
      <MachinePicker phone selected={homeHub} className="mx-3 mt-3 w-auto" />
      <nav aria-label="Settings categories" className="min-h-0 flex-1 overflow-y-auto p-2">
        {navCategories(discovery, hubDiscovery).map((category) => (
          <Link key={category.id} to="/settings/$tab" params={{ tab: navTabs(category, discovery, hubDiscovery)[0]!.id }} className={row}>
            <category.icon className="size-5 text-fg-secondary" />
            <span className="flex-1">{category.label}</span>
            <CategoryMark category={category} caps={caps} names={names} hubDiscovery={hubDiscovery} />
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
