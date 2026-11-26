import { AppWindow, ArrowLeft, X } from 'lucide-react'
import { notify as toast } from '@/lib/notify'
import { createContext, type ReactNode, useContext } from 'react'
import { SidebarToggle } from '@/components/chat/SidebarToggle'
import { Button } from '@/components/ui/button'
import { useSidebar } from '@/components/ui/sidebar'
import { cornerPad, headerToggle } from '@/lib/sidebar-corner'
import { useViewTitle } from '@danfessler/trellis-react'
import { useViewDrag } from '@/hooks/use-view-drag'
import { previewText } from '@/lib/channel-origin'
import { isMacDesktop } from '@/lib/hotkeys'
import { parseSpoken } from '@/lib/injected'
import { canPopout, isPopout } from '@/lib/popout'
import { cn } from '@/lib/utils'
import { parseNote } from '@/lib/voice-note'
import { useSessionField } from '@/store/sessions'
import { InsideTileCtx, InsideTileProvider, useInsideTile } from './tile-context'

export { InsideTileCtx, InsideTileProvider, useInsideTile }

/** A chat's title as its header shows it: a voice note's or spoken message's text, a channel message's preview. */
export const chatTitle = (raw: string): string => parseNote(raw)?.body || parseSpoken(raw)?.text || previewText(raw)

/**
 * Open in new window, then the tile leaves this layout by its own Close (`onClose`: the pane closes, its siblings
 * take the room, focus moves on — nothing else, its chat included, is touched). A window that failed to open leaves
 * the tile where it is.
 */
export async function popOutAndClose(popOut: () => Promise<unknown>, onClose: () => void): Promise<boolean> {
  try {
    await popOut()
  } catch (e) {
    toast.error(`Could not open a window: ${e instanceof Error ? e.message : String(e)}`)
    return false
  }
  onClose()
  return true
}

/** True under `PoppedTile`: the tile is its window's whole content (`routes/tile.$kind.$session`). */
export const TileAlone = createContext(false)

/**
 * Every generic tile's chrome: the 52 px header (the sidebar toggle and the traffic-light inset when `first`, like
 * `Header` and `WebFrame`; the title and its `meta` line; the kind's own `controls`; Open in new window when `popOut`
 * is given and this build has windows; Close — the last two not when the tile is its window alone, `TileAlone`) over
 * the kind's body. `onBack`, when given, puts a Back button before the title (a `view` tile with a page to go back
 * to). Beside other panes the header is the tile's drag handle (`useViewDrag`). Its Trellis tab (shown
 * while the tile shares a panel, e.g. with others of its kind) is named `tab`, else `title`; a kind a chat keeps one
 * of passes that chat as `chat`, so its tab carries the chat's title (two chats' Terminals tabs differ). Alone, the
 * header moves the window.
 */
export function TileFrame({
  title,
  tab,
  chat,
  meta,
  controls,
  popOut,
  onBack,
  onClose,
  first = false,
  children,
  className,
}: {
  title: string
  tab?: string
  chat?: string
  meta?: ReactNode
  controls?: ReactNode
  /** Opens the tile's own window; once that worked the tile closes here (its Close), a failure leaves it and toasts. */
  popOut?: () => Promise<unknown>
  /** Back within the tile's own history; omitted when there is nothing to go back to. */
  onBack?: () => void
  onClose: () => void
  first?: boolean
  children: ReactNode
  /** Merged onto the body (a flex column filling the rest of the tile). */
  className?: string
}) {
  const sidebar = useSidebar()
  const mac = isMacDesktop()
  // A tile alone in its own window (`PoppedTile`): no pop-out of itself, no Close (the OS title bar closes it). A tile
  // beside a popped chat keeps both.
  const popped = useContext(TileAlone)
  const nav = first && headerToggle(sidebar) && !isPopout()
  const drag = useViewDrag()
  const owner = useSessionField(chat ?? '', 'title')
  useViewTitle(owner ? `${tab ?? title} · ${chatTitle(owner)}` : (tab ?? title))
  const region = (mac && drag.region) || undefined
  return (
    <div className="flex min-h-0 min-w-0 flex-1 flex-col">
      <header
        {...drag.props}
        data-tauri-drag-region={region}
        data-tile-header=""
        className={cn('relative flex h-[52px] shrink-0 items-center gap-1 px-2', first && !isPopout() && mac && cornerPad(sidebar))}
      >
        {nav && <SidebarToggle />}
        {onBack && (
          <Button variant="ghost" size="icon" className="rounded-lg text-fg-secondary" aria-label="Back" onClick={onBack}>
            <ArrowLeft className="size-5" />
          </Button>
        )}
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
          <Button variant="ghost" className="size-9 rounded-lg text-fg-secondary" aria-label="Open in new window" onClick={() => void popOutAndClose(popOut, onClose)}>
            <AppWindow className="size-5" />
          </Button>
        )}
        {!popped && (
          <Button variant="ghost" size="icon" className="rounded-lg text-fg-secondary" aria-label="Close pane" onClick={onClose}>
            <X className="size-5" />
          </Button>
        )}
      </header>
      <div className={cn('relative flex min-h-0 min-w-0 flex-1 flex-col overflow-hidden border-t', className)}>
        <InsideTileProvider onClose={onClose}>{children}</InsideTileProvider>
      </div>
    </div>
  )
}
