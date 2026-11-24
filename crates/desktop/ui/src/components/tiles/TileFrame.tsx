import { AppWindow, X } from 'lucide-react'
import { createContext, type ReactNode, useContext } from 'react'
import { SidebarToggle } from '@/components/chat/SidebarToggle'
import { Button } from '@/components/ui/button'
import { useSidebar } from '@/components/ui/sidebar'
import { useViewTitle } from '@danfessler/trellis-react'
import { useViewDrag } from '@/hooks/use-view-drag'
import { isMacDesktop } from '@/lib/hotkeys'
import { canPopout, isPopout } from '@/lib/popout'
import { cn } from '@/lib/utils'

/** True under `PoppedTile`: the tile is its window's whole content (`routes/tile.$kind.$session`). */
export const TileAlone = createContext(false)

/**
 * Every generic tile's chrome: the 52 px header (the sidebar toggle and the traffic-light inset when `first`, like
 * `Header` and `WebFrame`; the title and its `meta` line; the kind's own `controls`; Open in new window when `popOut`
 * is given and this build has windows; Close — the last two not when the tile is its window alone, `TileAlone`) over
 * the kind's body. Beside other panes the header is the tile's drag handle (`useViewDrag`), and its title names the
 * tile's Trellis tab; alone, it moves the window.
 */
export function TileFrame({
  title,
  meta,
  controls,
  popOut,
  onClose,
  first = false,
  children,
  className,
}: {
  title: string
  meta?: ReactNode
  controls?: ReactNode
  popOut?: () => void
  onClose: () => void
  first?: boolean
  children: ReactNode
  /** Merged onto the body (a flex column filling the rest of the tile). */
  className?: string
}) {
  const { open, isMobile } = useSidebar()
  const mac = isMacDesktop()
  // A tile alone in its own window (`PoppedTile`): no pop-out of itself, no Close (the OS title bar closes it). A tile
  // beside a popped chat keeps both.
  const popped = useContext(TileAlone)
  const nav = first && (!open || isMobile) && !isPopout()
  const drag = useViewDrag()
  useViewTitle(title)
  const region = (mac && drag.region) || undefined
  return (
    <div className="flex min-h-0 min-w-0 flex-1 flex-col">
      <header
        {...drag.props}
        data-tauri-drag-region={region}
        data-tile-header=""
        className={cn('relative flex h-[52px] shrink-0 items-center gap-1 px-2', nav && !open && mac && 'pl-20')}
      >
        {nav && <SidebarToggle />}
        <div data-tauri-drag-region={region} className="flex min-w-0 flex-1 flex-col justify-center px-1">
          <span data-tauri-drag-region={region} className="truncate text-sm font-medium" title={title}>
            {title}
          </span>
          {meta && (
            <span data-tauri-drag-region={region} className="truncate text-xs text-fg-tertiary">
              {meta}
            </span>
          )}
        </div>
        {controls}
        {popOut && canPopout() && !popped && (
          <Button variant="ghost" className="size-9 rounded-lg text-fg-secondary" aria-label="Open in new window" onClick={popOut}>
            <AppWindow className="size-5" />
          </Button>
        )}
        {!popped && (
          <Button variant="ghost" size="icon" className="rounded-lg text-fg-secondary" aria-label="Close pane" onClick={onClose}>
            <X className="size-5" />
          </Button>
        )}
      </header>
      <div className={cn('relative flex min-h-0 min-w-0 flex-1 flex-col overflow-hidden border-t', className)}>{children}</div>
    </div>
  )
}
