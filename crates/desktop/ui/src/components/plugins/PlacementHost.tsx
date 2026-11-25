// 216 §9.1, §9.5, contract §12.5: where a plugin placement opens — a popover anchored to its icon on a desktop (the
// declared size, else 360 × 280, never past the window), a bottom sheet on a phone. The content mounts only while open,
// so opening fetches it (a lazy plugin is asked to render then).
import type { ReactNode } from 'react'
import { Popover, PopoverAnchor, PopoverContent, PopoverTrigger } from '@/components/ui/popover'
import { Sheet, SheetContent, SheetDescription, SheetTitle } from '@/components/ui/sheet'
import { useIsMobile } from '@/hooks/use-mobile'
import { usePluginSurface } from '@/lib/plugins'
import type { PlacementInfo } from '@/rpc/bindings/method.plugins.output'
import { PluginSurface } from './PluginSurface'

/** 216 §9.2: a popover's default size. */
export const POPOVER_SIZE = { width: 360, height: 280 } as const

export type PlacementHostProps = {
  plugin: string
  /** The placement whose content shows (a sidebar icon's own, or the tile its `open: tile` falls back from). */
  placement: PlacementInfo
  open: boolean
  onOpenChange(open: boolean): void
  /** The icon the popover hangs from. */
  children: ReactNode
  /** `children` only anchor the popover (an overflow menu's button, which opens its menu instead). */
  anchor?: boolean
  side?: 'right' | 'top' | 'bottom'
}

export function PlacementHost({ plugin, placement, open, onOpenChange, children, anchor = false, side = 'right' }: PlacementHostProps) {
  const mobile = useIsMobile()
  const size = placement.size ?? POPOVER_SIZE
  if (mobile)
    return (
      <>
        {children}
        <Sheet open={open} onOpenChange={onOpenChange}>
          <SheetContent side="bottom" className="flex max-h-[85%] min-h-48 flex-col gap-0 p-0 pt-2">
            <SheetTitle className="sr-only">{placement.label}</SheetTitle>
            <SheetDescription className="sr-only">plugin · {plugin}</SheetDescription>
            {open && <Content plugin={plugin} placement={placement} />}
          </SheetContent>
        </Sheet>
      </>
    )
  return (
    <Popover open={open} onOpenChange={onOpenChange}>
      {anchor ? <PopoverAnchor asChild>{children}</PopoverAnchor> : <PopoverTrigger asChild>{children}</PopoverTrigger>}
      <PopoverContent
        side={side}
        align="start"
        sideOffset={10}
        collisionPadding={8}
        aria-label={placement.label}
        style={{ width: size.width, height: size.height }}
        className="flex max-h-(--radix-popover-content-available-height) max-w-(--radix-popover-content-available-width) flex-col overflow-hidden rounded-xl p-0"
      >
        <Content plugin={plugin} placement={placement} />
      </PopoverContent>
    </Popover>
  )
}

function Content({ plugin, placement }: { plugin: string; placement: PlacementInfo }) {
  const { content } = usePluginSurface(plugin, placement.id, null)
  return <PluginSurface plugin={plugin} placement={placement.id} session={null} content={content} icon={placement.icon} />
}
