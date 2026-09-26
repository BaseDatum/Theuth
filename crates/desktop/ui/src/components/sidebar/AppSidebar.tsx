import { useQuery } from '@tanstack/react-query'
import { Link, useNavigate, useRouterState } from '@tanstack/react-router'
import { Bell, BookOpen, Bot, CalendarClock, FolderOpen, ListChecks, Lock, Megaphone, Mic, PanelLeft, Settings, Sparkles, SquarePen, StickyNote, Wrench } from 'lucide-react'
import { useEffect, useState } from 'react'
import { Mark } from '@/components/Mark'
import { EngineSwitcher } from '@/components/link/EngineSwitcher'
import { Button } from '@/components/ui/button'
import {
  Sidebar,
  SidebarContent,
  SidebarFooter,
  SidebarHeader,
  SidebarGroupLabel,
  SidebarMenu,
  SidebarMenuButton,
  SidebarMenuItem,
  useSidebar,
} from '@/components/ui/sidebar'
import { BroadcastDialog, openBroadcastDialog } from '@/components/chat/BroadcastDialog'
import { openNewChat } from '@/components/chat/NewChatPane'
import { useSessionDrag } from '@/components/chat/Panes'
import { AttentionBadge } from '@/components/hud/AttentionBadge'
import { settings } from '@/components/chat/slash'
import { THEUTH } from '@/components/tasks/helpers'
import { useAgents } from '@/components/tasks/queries'
import { isMacDesktop } from '@/lib/hotkeys'
import { label } from '@/lib/chord'
import { queryKeys, useApi } from '@/lib/rpc'
import { useShortcuts } from '@/lib/shortcuts'
import { useUpdate } from '@/lib/update'
import { cn } from '@/lib/utils'
import { useSessionField } from '@/store/sessions'
import { SearchTrigger } from './SearchTrigger'
import { SessionList } from './SessionList'

/** Injected by vite `define` from tauri.conf.json. */
declare const __APP_VERSION__: string

/** Top rail after New chat: who works (agents), on what (tasks), when (routines). */
const TOP = [
  ['/agents', 'Agents', Bot],
  ['/tasks', 'Tasks', ListChecks],
  ['/routines', 'Routines', CalendarClock],
] as const

/** The Library group: what the harness knows, keeps, heard, can use, holds and reaches. Artifacts stays last: this is its only nav entry. */
const LIBRARY = [
  ['/knowledge', 'Knowledge', BookOpen],
  ['/notes', 'Notes', StickyNote],
  ['/meetings', 'Meetings', Mic],
  ['/skills', 'Skills', Sparkles],
  ['/secrets', 'Secrets', Lock],
  ['/tools', 'Tools', Wrench],
  ['/artifacts', 'Artifacts', FolderOpen],
] as const

/** One row per roster agent with an open desk (`AgentSummary.desk`, else the pinned root titled `agent:<name>`): theuth first, then roster order. */
function Desks() {
  const api = useApi()
  const agents = useAgents().data ?? []
  const { data: sessions = [] } = useQuery({ queryKey: queryKeys.sessions, queryFn: () => api.list_sessions() })
  const desks = agents
    .filter((a) => a.status !== 'proposed' && a.status !== 'invalid')
    .flatMap((a) => {
      const id = a.desk ?? sessions.find((s) => s.pinned && s.parent === null && s.title === `agent:${a.file.name}`)?.id
      return id === undefined ? [] : [{ id, name: a.file.name, working: a.status === 'working' }]
    })
    .sort((x, y) => Number(y.name === THEUTH) - Number(x.name === THEUTH))
  if (desks.length === 0) return null
  return (
    <>
      <SidebarGroupLabel className="mt-2">Desks</SidebarGroupLabel>
      {desks.map((d) => (
        <DeskRow key={d.id} {...d} />
      ))}
    </>
  )
}

/** Opens like any chat; the dot is green while the roster says working or the session has a turn in flight. */
function DeskRow({ id, name, working }: { id: string; name: string; working: boolean }) {
  const running = useSessionField(id, 'running')
  return (
    <SidebarMenuItem>
      <SidebarMenuButton asChild className="h-9 rounded-[10px] px-2.5">
        <Link to="/c/$session" params={{ session: id }}>
          <span className={cn('size-2 shrink-0 rounded-full', working || running ? 'bg-success' : 'bg-fg-tertiary/40')} aria-hidden />
          {name}
        </Link>
      </SidebarMenuButton>
    </SidebarMenuItem>
  )
}

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
  // Slot for `SessionList`'s multi-select bar: a sibling below the scroller, so the bar stays put while the list scrolls.
  const [barEl, setBarEl] = useState<HTMLDivElement | null>(null)
  // 39 §4.2: the "subtle indicator" — a muted dot while an app update waits for quit.
  const pending = useUpdate((s) => s.pending)
  const newChat = useShortcuts((s) => s.chords['new-chat'])
  // Dragged onto a pane, New chat opens as a draft pane in the split (25 §12.3); a click opens the draft in the
  // focused pane when the view is split, else goes to `/` (the THEUTH mark above always does).
  const dragNew = useSessionDrag(null, 'New chat')

  return (
    <>
    <Sidebar collapsible="offcanvas" className="border-r-0">
      <SidebarHeader
        data-tauri-drag-region={isMacDesktop() || undefined}
        className={cn('h-[52px] flex-row items-center', isMacDesktop() && 'pl-20')}
      >
        <EngineSwitcher className="min-w-0 flex-1 justify-start" />
        <div className="ml-auto flex shrink-0 items-center">
          <SearchTrigger />
          <Button
            variant="ghost"
            size="icon"
            className="rounded-lg text-fg-secondary"
            aria-label="Broadcast…"
            title="Broadcast…"
            onClick={() => openBroadcastDialog({})}
          >
            <Megaphone className="size-5" />
          </Button>
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
              <Link
                to="/"
                {...dragNew}
                onClick={(e) => {
                  e.preventDefault()
                  openNewChat(navigate)
                }}
              >
                <SquarePen /> New chat
                <span className="ml-auto text-xs text-fg-tertiary">{label(newChat)}</span>
              </Link>
            </SidebarMenuButton>
          </SidebarMenuItem>
          {TOP.map(([to, name, Icon]) => (
            <SidebarMenuItem key={to}>
              <SidebarMenuButton asChild className="h-9 rounded-[10px] px-2.5">
                <Link to={to}>
                  <Icon /> {name}
                </Link>
              </SidebarMenuButton>
            </SidebarMenuItem>
          ))}
          <SidebarMenuItem><SidebarMenuButton asChild className="h-9 rounded-[10px] px-2.5"><Link to="/attention"><Bell /> Attention<AttentionBadge /></Link></SidebarMenuButton></SidebarMenuItem>
          <SidebarGroupLabel className="mt-2">Library</SidebarGroupLabel>
          {LIBRARY.map(([to, name, Icon]) => (
            <SidebarMenuItem key={to}>
              <SidebarMenuButton asChild className="h-9 rounded-[10px] px-2.5">
                <Link to={to}>
                  <Icon /> {name}
                </Link>
              </SidebarMenuButton>
            </SidebarMenuItem>
          ))}
          <Desks />
        </SidebarMenu>
        <SessionList scrollEl={scrollEl} barEl={barEl} active={session} />
      </SidebarContent>
      <div ref={setBarEl} className="shrink-0 empty:hidden" />
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
    {/* Beside, not inside: on phones the sidebar is a sheet that unmounts when closed (62 §5.2). */}
    <BroadcastDialog />
    </>
  )
}