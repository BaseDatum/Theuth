// 216 D11, §9.4: the host-owned strip above every plugin surface (popover, tile, command card): the plugin's icon and
// "plugin · <name>". Drawn outside the plugin's content tree, so the plugin can neither hide nor restyle it.
import { cn } from '@/lib/utils'
import { PluginIcon } from './icons'

export function Attribution({ plugin, icon = 'puzzle', className }: { plugin: string; icon?: string; className?: string }) {
  return (
    <div data-plugin-attribution className={cn('flex shrink-0 items-center gap-1.5 text-[11px] leading-4 text-fg-tertiary select-none', className)}>
      <PluginIcon icon={icon} className="size-3" />
      <span className="truncate">plugin · {plugin}</span>
    </div>
  )
}
