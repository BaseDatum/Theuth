import { Outlet, createRootRoute, useNavigate, useParams } from '@tanstack/react-router'
import { type CSSProperties, useEffect, useState } from 'react'
import { openNewChat } from '@/components/chat/NewChatPane'
import { DragOverlay } from '@/components/chat/Panes'
import { StudyDraftBridge } from '@/components/autoresearch/SetupPane'
import { MeetingBar } from '@/components/meeting/MeetingBar'
import { settings } from '@/components/chat/slash'
import { RoutineDraftBridge } from '@/components/routines/RoutineChat'
import { AppSidebar } from '@/components/sidebar/AppSidebar'
import { SearchDialog } from '@/components/sidebar/SearchDialog'
import { SidebarInset, SidebarProvider, useSidebar } from '@/components/ui/sidebar'
import { useHotkeys } from '@/lib/hotkeys'
import type { SearchRequest } from '@/lib/search'
import { isPopout } from '@/lib/popout'
import { isIsland, isOverlay } from '@/lib/tauri'

/** 03 §0.1: the island webview gets no sidebar, search or hotkeys — just its route; the trail overlay (96 §5.3) likewise. A popout gets the routed column alone. */
export const Route = createRootRoute({ component: isIsland() || isOverlay() ? Outlet : isPopout() ? Popped : Root })

/** One chat or one artifact in its own window: the column without sidebar, search, drag targets or the meeting bar.
 * `SidebarProvider` stays (closed) because the chat header reads it. */
function Popped() {
  return (
    <SidebarProvider open={false} className="h-band min-h-0">
      <SidebarInset className="min-h-0 overflow-hidden pt-[var(--safe-top)] pr-[var(--safe-right)] pb-[var(--safe-bottom)] pl-[var(--safe-left)]">
        <Outlet />
      </SidebarInset>
    </SidebarProvider>
  )
}

const WIDTH = { '--sidebar-width': '260px' } as CSSProperties

/** The shell: 260 px sidebar (⇧⌘S) beside the routed column; the meeting bar closes the column while a meeting records.
 * Routes render only once connected, so `useApi` is safe below. */
function Root() {
  const { session } = useParams({ strict: false })
  const navigate = useNavigate()
  const [open, setOpen] = useState(true)
  useHotkeys()

  useEffect(() => {
    const toggle = () => setOpen((o) => !o)
    const fresh = () => openNewChat(navigate)
    const shortcuts = () => void settings(navigate, 'shortcuts', session)
    window.addEventListener('theuth:toggle-sidebar', toggle)
    window.addEventListener('theuth:new-chat', fresh)
    window.addEventListener('theuth:shortcuts', shortcuts)
    return () => {
      window.removeEventListener('theuth:toggle-sidebar', toggle)
      window.removeEventListener('theuth:new-chat', fresh)
      window.removeEventListener('theuth:shortcuts', shortcuts)
    }
  }, [navigate, session])

  return (
    <SidebarProvider open={open} onOpenChange={setOpen} style={WIDTH} className="h-band min-h-0">
      <AppSidebar session={session} />
      <SearchHost />
      <RoutineDraftBridge />
      <StudyDraftBridge />
      <DragOverlay />
      <SidebarInset className="min-h-0 overflow-hidden pt-[var(--safe-top)] pr-[var(--safe-right)] pb-[var(--safe-bottom)] pl-[var(--safe-left)]">
        <Outlet />
        <MeetingBar />
      </SidebarInset>
    </SidebarProvider>
  )
}

/**
 * The one search instance (49 §5.1): every trigger, ⌘K, `/search` and `/sessions` open it through `theuth:search`;
 * a `CustomEvent` carries the `openSearch` request, a plain `Event` means an ordinary open. Lives inside
 * `EngineProvider` (App mounts the router under it) but outside the sidebar's mobile Sheet, so opening can
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
    window.addEventListener('theuth:search', show)
    return () => window.removeEventListener('theuth:search', show)
  }, [isMobile, setOpenMobile])
  return <SearchDialog open={open} onOpenChange={setOpen} request={request} />
}
