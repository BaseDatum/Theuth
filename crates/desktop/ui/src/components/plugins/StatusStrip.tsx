// 216 §9.1, contract §12.5: the `status` placements — icon and dot in the status bar's right slot, label and tooltip on
// hover, at most three, the rest behind an overflow menu that lights when one of them warns. Not on a phone: there the
// sidebar sheet's footer lists them (`variant="sheet"`), labels shown since a finger cannot hover.
import { Ellipsis } from 'lucide-react'
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuTrigger } from '@/components/ui/dropdown-menu'
import { useIsMobile } from '@/hooks/use-mobile'
import { type PlacementEntry, badgeText, entryDot, entryTip, overflowDot, placementKey, usePluginPlacements } from '@/lib/plugins'
import { useQuerySupport } from '@/rpc/capabilities'
import { PluginIcon } from './icons'
import { PluginDot } from './PluginDot'

/** Contract §14.8: status items shown before the overflow menu. */
export const STATUS_CAP = 3

type Variant = 'bar' | 'sheet'

/** `bar`: the desktop status bar (nothing on a phone). `sheet`: the phone's sidebar footer (nothing elsewhere). */
export function StatusStrip({ variant = 'bar' }: { variant?: Variant }) {
  const mobile = useIsMobile()
  const able = useQuerySupport(['plugin_list', 'plugin_surfaces'])
  if (!able || mobile !== (variant === 'sheet')) return null
  return <Strip variant={variant} />
}

function Strip({ variant }: { variant: Variant }) {
  const entries = usePluginPlacements('status')
  if (entries.length === 0) return null
  if (variant === 'sheet')
    return (
      <div data-plugin-status="sheet" className="flex flex-wrap items-center gap-x-3 gap-y-1 px-2.5 py-1 text-xs text-fg-tertiary">
        {entries.map((e) => (
          <Item key={placementKey(e.plugin.name, e.placement.id)} entry={e} labelled />
        ))}
      </div>
    )
  const shown = entries.slice(0, STATUS_CAP)
  const rest = entries.slice(STATUS_CAP)
  return (
    <span data-plugin-status="bar" className="flex shrink-0 items-center gap-2.5">
      {shown.map((e) => (
        <Item key={placementKey(e.plugin.name, e.placement.id)} entry={e} />
      ))}
      {rest.length > 0 && <Overflow entries={rest} />}
    </span>
  )
}

function Item({ entry, labelled = false }: { entry: PlacementEntry; labelled?: boolean }) {
  const tip = entryTip(entry)
  const badge = badgeText(entry.status?.badge)
  return (
    <span data-plugin={entry.plugin.name} className="flex min-w-0 shrink-0 items-center gap-1" title={tip} aria-label={tip}>
      <PluginIcon icon={entry.placement.icon} className="size-3" />
      <PluginDot dot={entryDot(entry)} />
      {labelled && <span className="truncate">{entry.status?.label ?? entry.placement.label}</span>}
      {badge !== null && <span className="font-mono">{badge}</span>}
    </span>
  )
}

function Overflow({ entries }: { entries: readonly PlacementEntry[] }) {
  const label = `${entries.length} more plugin${entries.length === 1 ? '' : 's'}`
  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <button type="button" aria-label={label} title={label} data-plugin-overflow="" className="relative flex h-full shrink-0 items-center gap-1 hover:text-foreground">
          <Ellipsis className="size-3" />
          <PluginDot dot={overflowDot(entries)} />
        </button>
      </DropdownMenuTrigger>
      <DropdownMenuContent side="top" align="end" className="min-w-44 text-xs">
        {entries.map((e) => (
          <DropdownMenuItem key={placementKey(e.plugin.name, e.placement.id)} title={entryTip(e)} className="text-xs">
            <PluginIcon icon={e.placement.icon} className="size-3.5" />
            <span className="truncate">{entryTip(e)}</span>
            <PluginDot dot={entryDot(e)} className="ml-auto" />
          </DropdownMenuItem>
        ))}
      </DropdownMenuContent>
    </DropdownMenu>
  )
}
