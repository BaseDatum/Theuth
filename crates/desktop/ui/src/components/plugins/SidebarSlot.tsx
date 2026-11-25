// 216 §9.1, contract §12.5: the `sidebar` placements — icons in the collapsed rail (after the sections) and in a row of
// the expanded sidebar (between the desks and the chat list). Dot and badge on each icon; at most five, the rest in an
// overflow menu whose button lights when one of them warns. A click opens the placement's popover (a bottom sheet on a
// phone) or, for `open: tile`, the plugin's tile beside the focused chat — the popover when no chat pane can take it.
// The context menu hides a placement (`plugin_set_hidden`).
import { Ellipsis, EyeOff } from 'lucide-react'
import { Fragment, type MouseEvent, type ReactNode, useState } from 'react'
import { RAIL_ITEM } from '@/components/sidebar/Rail'
import { openPluginTile } from '@/components/tiles/open'
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'
import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip'
import { notify as toast } from '@/lib/notify'
import { usePanes } from '@/lib/panes'
import { type PlacementEntry, badgeText, entryDot, entryTip, overflowDot, placementKey, usePluginPlacements } from '@/lib/plugins'
import { tileable } from '@/lib/tiles'
import { cn } from '@/lib/utils'
import { useQuerySupport } from '@/rpc/capabilities'
import type { Dot, PlacementInfo } from '@/rpc/bindings/method.plugins.output'
import { useConnection } from '@/store/connection'
import { PluginIcon } from './icons'
import { PlacementHost } from './PlacementHost'
import { PluginDot } from './PluginDot'

/** Contract §14.8: sidebar icons shown before the overflow menu. */
export const SIDEBAR_CAP = 5

type Variant = 'rail' | 'row'

const ROW_ITEM = cn(
  'relative flex size-8 shrink-0 items-center justify-center rounded-[10px] text-fg-secondary ring-sidebar-ring outline-hidden pointer-coarse:size-10',
  'hover:bg-sidebar-accent hover:text-sidebar-accent-foreground focus-visible:ring-2 data-[state=open]:bg-sidebar-accent data-[state=open]:text-sidebar-accent-foreground',
  '[&_svg]:size-4',
)

/** What the entry's popover shows: its own content, or (`open: tile`) the tile it would open. */
const surfaceOf = (e: PlacementEntry): PlacementInfo =>
  (e.placement.open === 'tile' && e.plugin.placements.find((p) => p.kind === 'tile')) || e.placement

/** `open: tile`: the plugin's tile beside the focused chat; false when nothing tiles here (the caller opens the popover). */
function openTileHere(e: PlacementEntry): boolean {
  if (e.placement.open !== 'tile') return false
  const tile = e.plugin.placements.find((p) => p.kind === 'tile')
  const host = usePanes.getState().focused
  return !!tile && host !== null && tileable(host) && openPluginTile(host, { plugin: e.plugin.name, placement: tile.id }) !== null
}

export function SidebarSlot({ variant }: { variant: Variant }) {
  const able = useQuerySupport(['plugin_list', 'plugin_surfaces', 'plugin_surface'])
  return able ? <Slot variant={variant} /> : null
}

function Slot({ variant }: { variant: Variant }) {
  const entries = usePluginPlacements('sidebar')
  /** The entry whose popover is open (shown or overflowed). */
  const [open, setOpen] = useState<string | null>(null)
  if (entries.length === 0) return null
  const shown = entries.slice(0, SIDEBAR_CAP)
  const rest = entries.slice(SIDEBAR_CAP)
  const keyOf = (e: PlacementEntry) => placementKey(e.plugin.name, e.placement.id)
  const toggle = (e: PlacementEntry) => (on: boolean) => setOpen(on ? keyOf(e) : null)
  const items = shown.map((e) => <Item key={keyOf(e)} entry={e} variant={variant} open={open === keyOf(e)} onOpenChange={toggle(e)} />)
  const picked = rest.find((e) => keyOf(e) === open)
  const overflow = rest.length > 0 && (
    <Overflow
      key="overflow"
      entries={rest}
      variant={variant}
      picked={picked}
      onPick={(e) => !openTileHere(e) && setOpen(keyOf(e))}
      onClose={() => setOpen(null)}
    />
  )
  if (variant === 'rail')
    return (
      <div data-plugin-slot="rail" className="flex w-full flex-col items-center gap-1">
        <div role="separator" className="my-1.5 h-px w-8 shrink-0 bg-sidebar-border" />
        {items}
        {overflow}
      </div>
    )
  return (
    <div data-plugin-slot="row" aria-label="Plugins" className="flex flex-wrap items-center gap-0.5 px-2 pt-2 pb-1">
      {items}
      {overflow}
    </div>
  )
}

/** A rail icon's name to its right; a row icon's below it. */
function Tip({ variant, label, children }: { variant: Variant; label: string; children: ReactNode }) {
  return (
    <Tooltip>
      {children}
      <TooltipContent side={variant === 'rail' ? 'right' : 'bottom'}>{label}</TooltipContent>
    </Tooltip>
  )
}

function Marks({ dot, badge }: { dot: Dot; badge: string | null }) {
  return (
    <>
      {badge !== null && (
        <span className="absolute -top-0.5 -right-0.5 h-4 min-w-4 rounded-full bg-warning px-1 text-center text-[10px] leading-4 font-semibold tabular-nums text-primary-foreground ring-2 ring-sidebar">
          {badge}
        </span>
      )}
      <PluginDot dot={dot} className={cn('absolute size-2 ring-2 ring-sidebar', badge === null ? 'top-1.5 right-1.5' : 'right-1.5 bottom-1.5')} />
    </>
  )
}

function Item({ entry, variant, open, onOpenChange }: { entry: PlacementEntry; variant: Variant; open: boolean; onOpenChange(on: boolean): void }) {
  const api = useConnection((s) => s.api)
  const [menu, setMenu] = useState(false)
  const { plugin, placement } = entry
  const tip = entryTip(entry)
  const click = (ev: MouseEvent) => {
    // Ours, not the popover trigger's toggle: a tile may open instead, and a phone has no trigger (a sheet).
    ev.preventDefault()
    if (openTileHere(entry)) return
    onOpenChange(!open)
  }
  const hide = () => {
    api?.plugin_set_hidden(plugin.name, placement.id, true).catch((e: unknown) => toast.error(e instanceof Error ? e.message : String(e)))
  }
  return (
    <span
      className="relative inline-flex shrink-0"
      onContextMenu={(ev) => {
        ev.preventDefault()
        setMenu(true)
      }}
    >
      <Tip variant={variant} label={tip}>
        <PlacementHost plugin={plugin.name} placement={surfaceOf(entry)} open={open} onOpenChange={onOpenChange}>
          <TooltipTrigger asChild>
            <button type="button" className={variant === 'rail' ? RAIL_ITEM : ROW_ITEM} aria-label={placement.label} data-plugin={plugin.name} onClick={click}>
              <PluginIcon icon={placement.icon} />
              <Marks dot={entryDot(entry)} badge={badgeText(entry.status?.badge)} />
            </button>
          </TooltipTrigger>
        </PlacementHost>
      </Tip>
      <DropdownMenu open={menu} onOpenChange={setMenu}>
        <DropdownMenuTrigger asChild>
          <span aria-hidden className="pointer-events-none absolute inset-0" />
        </DropdownMenuTrigger>
        <DropdownMenuContent side={variant === 'rail' ? 'right' : 'bottom'} align="start">
          <DropdownMenuLabel className="text-xs font-normal text-fg-tertiary">plugin · {plugin.name}</DropdownMenuLabel>
          <DropdownMenuItem onSelect={hide}>
            <EyeOff />
            Hide
          </DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>
    </span>
  )
}

function Overflow({
  entries,
  variant,
  picked,
  onPick,
  onClose,
}: {
  entries: readonly PlacementEntry[]
  variant: Variant
  picked: PlacementEntry | undefined
  onPick(e: PlacementEntry): void
  onClose(): void
}) {
  const label = `${entries.length} more plugin${entries.length === 1 ? '' : 's'}`
  const button = (
    <span className="relative inline-flex shrink-0">
      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          <button type="button" className={variant === 'rail' ? RAIL_ITEM : ROW_ITEM} aria-label={label} data-plugin-overflow="">
            <Ellipsis />
            <PluginDot dot={overflowDot(entries)} className="absolute top-1.5 right-1.5 size-2 ring-2 ring-sidebar" />
          </button>
        </DropdownMenuTrigger>
        {/* The picked popover hangs from this button: focus must not come back to it and close that popover. */}
        <DropdownMenuContent side={variant === 'rail' ? 'right' : 'bottom'} align="start" className="min-w-48" onCloseAutoFocus={(ev) => ev.preventDefault()}>
          {entries.map((e, i) => (
            <Fragment key={placementKey(e.plugin.name, e.placement.id)}>
              {i > 0 && e.plugin.name !== entries[i - 1]?.plugin.name && <DropdownMenuSeparator />}
              <DropdownMenuItem onSelect={() => onPick(e)} title={entryTip(e)}>
                <PluginIcon icon={e.placement.icon} />
                <span className="truncate">{e.placement.label}</span>
                <span className="ml-auto flex items-center gap-1.5 pl-3">
                  {badgeText(e.status?.badge) !== null && <span className="font-mono text-[11px] text-fg-tertiary">{badgeText(e.status?.badge)}</span>}
                  <PluginDot dot={entryDot(e)} className="size-2" />
                </span>
              </DropdownMenuItem>
            </Fragment>
          ))}
        </DropdownMenuContent>
      </DropdownMenu>
    </span>
  )
  if (!picked) return button
  return (
    <PlacementHost plugin={picked.plugin.name} placement={surfaceOf(picked)} open onOpenChange={(on) => !on && onClose()} anchor>
      {button}
    </PlacementHost>
  )
}
