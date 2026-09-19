import { Outlet, createRootRoute, useNavigate, useParams } from '@tanstack/react-router'
import { type CSSProperties, useEffect, useState } from 'react'
import { DragOverlay } from '@/components/chat/Panes'
import { settings } from '@/components/chat/slash'
import { AppSidebar } from '@/components/sidebar/AppSidebar'
import { SearchDialog } from '@/components/sidebar/SearchDialog'
import { SidebarInset, SidebarProvider, useSidebar } from '@/components/ui/sidebar'
import { useHotkeys } from '@/lib/hotkeys'
import type { SearchRequest } from '@/lib/search'

export const Route = createRootRoute({ component: Root })

const WIDTH = { '--sidebar-width': '260px' } as CSSProperties

/** The shell: 260 px sidebar (⇧⌘S) beside the routed column. Routes render only once connected, so `useApi` is safe below. */
function Root() {
  const { session } = useParams({ strict: false })
  const navigate = useNavigate()
  const [open, setOpen] = useState(true)
  useHotkeys()

  useEffect(() => {
    const toggle = () => setOpen((o) => !o)
    const fresh = () => void navigate({ to: '/' })
    const shortcuts = () => void settings(navigate, 'shortcuts', session)
    window.addEventListener('wth:toggle-sidebar', toggle)
    window.addEventListener('wth:new-chat', fresh)
    window.addEventListener('wth:shortcuts', shortcuts)
    return () => {
      window.removeEventListener('wth:toggle-sidebar', toggle)
      window.removeEventListener('wth:new-chat', fresh)
      window.removeEventListener('wth:shortcuts', shortcuts)
    }
  }, [navigate, session])

  return (
    <SidebarProvider open={open} onOpenChange={setOpen} style={WIDTH} className="h-band min-h-0">
      <AppSidebar session={session} />
      <SearchHost />
      <DragOverlay />
      <SidebarInset className="min-h-0 overflow-hidden pt-[var(--safe-top)] pr-[var(--safe-right)] pb-[var(--safe-bottom)] pl-[var(--safe-left)]">
        <Outlet />
      </SidebarInset>
    </SidebarProvider>
  )
}

/**
 * The one search instance (49 §5.1): every trigger, ⌘K, `/search` and `/sessions` open it through `wth:search`;
 * a `CustomEvent` carries the `openSearch` request, a plain `Event` means an ordinary open. Lives inside
 * `HarnessProvider` (App mounts the router under it) but outside the sidebar's mobile Sheet, so opening can
 * dismiss the drawer without unmounting the dialog — one modal, one focus trap. Focus return is the dialog's.
 */
function SearchHost() {
  const { isMobile, setOpenMobile } = useSidebar()
  const [open, setOpen] = useState(false)
  const [request, setRequest] = useState<SearchRequest>()
  useEffect(() => {
    const show = (e: Event) => {
      if (isMobile) setOpenMobile(false)
      setRequest({ ...((e as CustomEvent<SearchRequest | undefined>).detail ?? {}) })
      setOpen(true)
    }
    window.addEventListener('wth:search', show)
    return () => window.removeEventListener('wth:search', show)
  }, [isMobile, setOpenMobile])
  return <SearchDialog open={open} onOpenChange={setOpen} request={request} />
}
