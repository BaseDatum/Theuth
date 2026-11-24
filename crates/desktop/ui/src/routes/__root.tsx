import { Capability } from '@/rpc/capabilities'
import { requirements as routineRequirements } from '@/components/routines/requirements'
import { requirements as studyRequirements } from '@/components/autoresearch/requirements'
import { requirements as costsRequirements } from '@/components/costs/requirements'
import { requirements as meetingRequirements } from '@/components/meeting/requirements'
import { searchRequirements } from '@/components/sidebar/requirements'
import { Outlet, createRootRoute, useNavigate, useParams, useRouterState } from '@tanstack/react-router'
import { useEffect, useState } from 'react'
import { openNewChat } from '@/components/chat/NewChatPane'
import { DragOverlay } from '@/components/chat/Panes'
import { StudyDraftBridge } from '@/components/autoresearch/SetupPane'
import { MeetingBar } from '@/components/meeting/MeetingBar'
import { settings } from '@/components/chat/slash'
import { PublicShareHost } from '@/components/public-share/PublicShareHost'
import { PublicShareStrip } from '@/components/public-share/PublicShareStrip'
import { RoutineDraftBridge } from '@/components/routines/RoutineChat'
import { AppSidebar } from '@/components/sidebar/AppSidebar'
import { SearchDialog } from '@/components/sidebar/SearchDialog'
import { SidebarInset, SidebarProvider, readSidebarOpen, useSidebar, writeSidebarOpen } from '@/components/ui/sidebar'
import { useHotkeys } from '@/lib/hotkeys'
import { CostsPrefetch } from '@/lib/costs/queries'
import { CostsResident, ResidentCtx } from '@/components/costs/Resident'
import type { SearchRequest } from '@/lib/search'
import { pageTitle } from '@/lib/nav'
import { POPOUT, isPopout } from '@/lib/popout'
import { isIsland, isOverlay } from '@/lib/tauri'

/** 03 §0.1: the island webview gets no sidebar, search or hotkeys — just its route; the trail overlay (96 §5.3) likewise. A popout gets the routed column alone. */
export const Route = createRootRoute({ component: isIsland() || isOverlay() ? Outlet : isPopout() ? Popped : Root })

/** One chat, one artifact or one sidebar page (`page`) in its own window: the column without sidebar, search, drag
 * targets or the meeting bar. `SidebarProvider` stays because the headers read it, `open` because there is no rail to
 * reopen: no page shows a "Show sidebar" toggle or clears traffic lights (a popped window has its own OS title bar). */
function Popped() {
  const pathname = useRouterState({ select: (s) => s.location.pathname })
  useEffect(() => {
    const title = POPOUT?.kind === 'page' ? pageTitle(pathname) : undefined
    if (title !== undefined) document.title = title
  }, [pathname])
  return (
    <SidebarProvider open className="h-band min-h-0">
      {/* A window has its own query cache: a page window (Costs among its pages) loads the cube itself; the shell's
       * resident costs host stays out, so `/costs` renders the page in the column. */}
      {POPOUT?.kind === 'page' && <Capability required={costsRequirements} hide><CostsPrefetch /></Capability>}
      <SidebarInset className="min-h-0 overflow-hidden pt-[var(--safe-top)] pr-[var(--safe-right)] pb-[var(--safe-bottom)] pl-[var(--safe-left)]">
        <Outlet />
      </SidebarInset>
    </SidebarProvider>
  )
}

/** The shell: resizable sidebar (⇧⌘S) beside the routed column; the meeting bar closes the column while a meeting records.
 * On a desktop it collapses to the icon rail, never away; expanded or rail is kept in the `sidebar_state` cookie.
 * Routes render only once connected, so `useApi` is safe below. */
function Root() {
  const { session } = useParams({ strict: false })
  const navigate = useNavigate()
  const [open, setOpen] = useState(() => readSidebarOpen(globalThis.document?.cookie ?? ''))
  useHotkeys()
  // Every path that changes it (the toggles, ⌘B, ⇧⌘S's event, a swipe) lands here.
  useEffect(() => writeSidebarOpen(open), [open])

  useEffect(() => {
    const toggle = () => setOpen((o) => !o)
    const fresh = () => openNewChat(navigate)
    const shortcuts = () => void settings(navigate, 'shortcuts')
    window.addEventListener('theuth:toggle-sidebar', toggle)
    window.addEventListener('theuth:new-chat', fresh)
    window.addEventListener('theuth:shortcuts', shortcuts)
    return () => {
      window.removeEventListener('theuth:toggle-sidebar', toggle)
      window.removeEventListener('theuth:new-chat', fresh)
      window.removeEventListener('theuth:shortcuts', shortcuts)
    }
  }, [navigate])

  return (
    <SidebarProvider open={open} onOpenChange={setOpen} rail className="h-band min-h-0">
      <AppSidebar session={session} />
      <Capability required={searchRequirements} hide><SearchHost /></Capability>
      <Capability required={routineRequirements} hide><RoutineDraftBridge /></Capability>
      <Capability required={costsRequirements} hide><CostsPrefetch /></Capability>
      <Capability required={studyRequirements} hide><StudyDraftBridge /></Capability>
      <DragOverlay />
      <PublicShareHost />
      <SidebarInset className="min-h-0 overflow-hidden pt-[var(--safe-top)] pr-[var(--safe-right)] pb-[var(--safe-bottom)] pl-[var(--safe-left)]">
        {/* 210 §7.4 item 1: over every route while anything of the mesh is public. */}
        <PublicShareStrip />
        {/* 160 R2: the routed page and the resident costs page share one column; the costs page lies over it. */}
        <ResidentCtx value>
          <div className="relative flex min-h-0 flex-1 flex-col">
            <Outlet />
            <Capability required={costsRequirements} hide><CostsResident /></Capability>
          </div>
        </ResidentCtx>
        <Capability required={meetingRequirements} hide><MeetingBar /></Capability>
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
