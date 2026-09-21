import { Link, useNavigate, useRouterState } from '@tanstack/react-router'
import { BookOpen, FolderOpen, Mic, PanelLeft, Settings, SquarePen, StickyNote } from 'lucide-react'
import { useEffect, useState } from 'react'
import { Mark } from '@/components/Mark'
import { HarnessSwitcher } from '@/components/link/HarnessSwitcher'
import { Button } from '@/components/ui/button'
import {
  Sidebar,
  SidebarContent,
  SidebarFooter,
  SidebarHeader,
  SidebarMenu,
  SidebarMenuButton,
  SidebarMenuItem,
  useSidebar,
} from '@/components/ui/sidebar'
import { settings } from '@/components/chat/slash'
import { isMacDesktop } from '@/lib/hotkeys'
import { label } from '@/lib/chord'
import { useShortcuts } from '@/lib/shortcuts'
import { useUpdate } from '@/lib/update'
import { cn } from '@/lib/utils'
import { SearchTrigger } from './SearchTrigger'
import { SessionList } from './SessionList'

/** Injected by vite `define` from tauri.conf.json. */
declare const __APP_VERSION__: string

/** ChatGPT's left rail: New chat, Knowledge, Artifacts, the chat list, Settings; Search sits in the header beside the toggle (49 §5.1). */
export function AppSidebar({ session }: { session: string | undefined }) {
  const navigate = useNavigate()
  const { isMobile, setOpenMobile, toggleSidebar } = useSidebar()
  // Phones: the sidebar is a sheet over the chat, so every tap that navigates (a chat, New chat,
  // Knowledge, Settings) also dismisses it. Selecting rows for deletion does not navigate.
  const href = useRouterState({ select: (s) => s.location.href })
  useEffect(() => {
    if (isMobile) setOpenMobile(false)
  }, [href, isMobile, setOpenMobile])
  // State, not a ref: `SessionList`'s virtualizer reads the scroll element in its layout effect,
  // which runs before this parent's ref would attach; the state change re-renders it with the element.
  const [scrollEl, setScrollEl] = useState<HTMLDivElement | null>(null)
  // 39 §4.2: the "subtle indicator" — a muted dot while an app update waits for quit.
  const pending = useUpdate((s) => s.pending)
  const newChat = useShortcuts((s) => s.chords['new-chat'])

  return (
    <Sidebar collapsible="offcanvas" className="border-r-0">
      <SidebarHeader
        data-tauri-drag-region={isMacDesktop() || undefined}
        className={cn('h-[52px] flex-row items-center', isMacDesktop() && 'pl-20')}
      >
        <HarnessSwitcher className="min-w-0 flex-1 justify-start" />
        <div className="ml-auto flex shrink-0 items-center">
          <SearchTrigger />
          <Button
            variant="ghost"
            size="icon"
            className="rounded-lg text-fg-secondary"
            aria-label="Close sidebar"
            onClick={toggleSidebar}
          >
            <PanelLeft className="size-5" />
          </Button>
        </div>
      </SidebarHeader>
      <SidebarContent ref={setScrollEl} className="gap-0 overflow-y-auto">
        <SidebarMenu className="px-1.5">
          <SidebarMenuItem>
            <SidebarMenuButton asChild className="h-9 rounded-[10px] px-2.5 font-semibold tracking-[.08em]">
              <Link to="/" aria-label="theuth home">
                <Mark /> THEUTH
              </Link>
            </SidebarMenuButton>
          </SidebarMenuItem>
          <SidebarMenuItem>
            <SidebarMenuButton asChild className="h-9 rounded-[10px] px-2.5">
              <Link to="/">
                <SquarePen /> New chat
                <span className="ml-auto text-xs text-fg-tertiary">{label(newChat)}</span>
              </Link>
            </SidebarMenuButton>
          </SidebarMenuItem>
          <SidebarMenuItem>
            <SidebarMenuButton asChild className="h-9 rounded-[10px] px-2.5">
              <Link to="/knowledge">
                <BookOpen /> Knowledge
              </Link>
            </SidebarMenuButton>
          </SidebarMenuItem>
          <SidebarMenuItem>
            <SidebarMenuButton asChild className="h-9 rounded-[10px] px-2.5">
              <Link to="/notes">
                <StickyNote /> Notes
              </Link>
            </SidebarMenuButton>
          </SidebarMenuItem>
          <SidebarMenuItem>
            <SidebarMenuButton asChild className="h-9 rounded-[10px] px-2.5">
              <Link to="/artifacts">
                <FolderOpen /> Artifacts
              </Link>
            </SidebarMenuButton>
          </SidebarMenuItem>
          <SidebarMenuItem>
            <SidebarMenuButton asChild className="h-9 rounded-[10px] px-2.5">
              <Link to="/meetings">
                <Mic /> Meetings
              </Link>
            </SidebarMenuButton>
          </SidebarMenuItem>
        </SidebarMenu>
        <SessionList scrollEl={scrollEl} active={session} />
      </SidebarContent>
      <SidebarFooter className="px-1.5">
        <SidebarMenu>
          <SidebarMenuItem>
            <SidebarMenuButton
              className="h-9 rounded-[10px] px-2.5"
              onClick={() => void settings(navigate, 'general', session)}
            >
              <Settings /> Settings
              <span className="ml-auto text-xs text-fg-tertiary">v{__APP_VERSION__}</span>
              {pending !== null && <span className="size-1.5 rounded-full bg-fg-tertiary" aria-label="update ready" />}
            </SidebarMenuButton>
          </SidebarMenuItem>
        </SidebarMenu>
      </SidebarFooter>
    </Sidebar>
  )
}
