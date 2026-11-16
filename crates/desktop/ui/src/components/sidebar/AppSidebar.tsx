import { Capability } from '@/rpc/capabilities'
import { supports } from '@/rpc/discovery'
import { useConnection } from '@/store/connection'
import { GROUPS } from '@/lib/nav'
import { requirements as talkRequirements } from '@/components/talk/requirements'
import { requirements as noteRequirements } from '@/components/notes/requirements'
import { requirements as attentionRequirements } from '@/components/attention/requirements'
import { requirements as costsRequirements } from '@/components/costs/requirements'
import { broadcastRequirements } from '@/components/chat/requirements'
import { useMutation, useQueryClient } from '@tanstack/react-query'
import { Link, useNavigate, useRouterState } from '@tanstack/react-router'
import { AudioLines, Bell, Bot, ChartColumn, Library, Megaphone, Mic, PanelLeft, Plus, Settings, SquarePen, StickyNote } from 'lucide-react'
import { useEffect, useState } from 'react'
import { toast } from 'sonner'
import { Mark } from '@/components/Mark'
import { EngineSwitcher } from '@/components/link/EngineSwitcher'
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
import { BroadcastDialog, openBroadcastDialog } from '@/components/chat/BroadcastDialog'
import { NewChatOnMenu, openNewChat } from '@/components/chat/NewChatPane'
import { createNote } from '@/components/notes/create'
import { useSessionDrag } from '@/components/chat/Panes'
import { AttentionBadge } from '@/components/hud/AttentionBadge'
import { useCanRecord } from '@/components/meeting/MeetingList'
import { settings } from '@/components/chat/slash'
import { errorText } from '@/components/settings/shared'
import { isMacDesktop } from '@/lib/hotkeys'
import { sectionOf } from '@/lib/nav'
import { canPopout, noteOpenMode, openPopout } from '@/lib/popout'
import { label } from '@/lib/chord'
import { queryKeys, useApi } from '@/lib/rpc'
import { useShortcuts } from '@/lib/shortcuts'
import * as talk from '@/lib/talk'
import { useUpdate } from '@/lib/update'
import { cn } from '@/lib/utils'
import type { DocSummary } from "@/rpc/bindings/method.agent.output"
import { Desks } from './Desks'
import { SearchTrigger } from './SearchTrigger'
import { SessionList } from './SessionList'
import { SidebarPopoutButton } from './SidebarPopoutButton'

/** Injected by vite `define` from tauri.conf.json. */
declare const __APP_VERSION__: string

/** 143 D2: the plus on the Note row — malachite (`--status-success-fg`); one token to swap for `text-project-nile` (olive). */
export const NOTE_PLUS = 'text-success'

/**
 * ChatGPT's left rail: Chat, Talk, Note, Meeting, Attention, Agents, Library, Costs, the desks, the chat list, Settings; Search sits in
 * the header beside the toggle (49 §5.1). 97: the five Agents pages and the five Library pages are one row each; the
 * page under it carries the tab band (`SectionBand`), so the rail stays short.
 */
export function AppSidebar({ session }: { session: string | undefined }) {
  const discovery = useConnection(s => s.discovery)
  const agentsTab = GROUPS.agents.find(tab => supports(discovery, tab.required))
  const libraryTab = GROUPS.library.find(tab => supports(discovery, tab.required))
  const navigate = useNavigate()
  const { isMobile, setOpenMobile, toggleSidebar } = useSidebar()
  // Phones: the sidebar is a sheet over the chat, so every tap that navigates (a chat, New chat,
  // Knowledge, Settings) also dismisses it. Selecting rows for deletion does not navigate.
  const href = useRouterState({ select: (s) => s.location.href })
  const section = sectionOf(useRouterState({ select: (s) => s.location.pathname }))
  // 97: the Meeting row starts a recording, which only this Mac's own daemon can do; elsewhere the row is noise.
  const canRecord = useCanRecord()
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
  const api = useApi()
  const queryClient = useQueryClient()
  // 76 §0.12: the live dot while a talk runs (anywhere); the entry opens the running talk rather than starting a second.
  const talking = talk.useTalk()
  const live = talking.phase !== 'off'
  // B3: talk is a mode of the session — from a chat (or its talk view) the entry talks there; only elsewhere does it open a new session.
  const current = /^\/(?:c|talk)\/([^/?#]+)/.exec(href)?.[1] ?? null
  // Dragged onto a pane, New chat opens as a draft pane in the split (25 §12.3); a click opens the draft in the
  // focused pane when the view is split, else goes to `/` (the THEUTH mark above always does).
  const dragNew = useSessionDrag(null, 'New chat')
  // 143 D3: Note creates then opens — its own window on a desktop (the shell stays put), the note page on a phone/iOS
  // (the sheet closes on the href change above). Reads the notes cache rather than `useNotes()`: no permanent subscription.
  const createNoteAndOpen = useMutation({
    mutationFn: () => createNote(api, queryClient, queryClient.getQueryData<DocSummary[]>(queryKeys.notes) ?? []),
    onSuccess: (n) => {
      if (noteOpenMode(isMobile, canPopout()) === 'popout') void openPopout({ kind: 'note', doc: n.id, fresh: true })
      else void navigate({ to: '/notes/$id', params: { id: n.id }, search: { new: true } })
    },
    onError: (e) => toast.error(errorText(e)),
  })

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
          <Capability required={broadcastRequirements} hide><Button
            variant="ghost"
            size="icon"
            className="rounded-lg text-fg-secondary"
            aria-label="Broadcast…"
            title="Broadcast…"
            onClick={() => openBroadcastDialog({})}
          >
            <Megaphone className="size-5" />
          </Button></Capability>
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
            <SidebarMenuButton asChild isActive={section === 'home'} className="h-9 rounded-[10px] px-2.5 font-semibold tracking-[.08em]">
              <Link to="/" aria-label="theuth home">
                <Mark /> THEUTH
              </Link>
            </SidebarMenuButton>
          </SidebarMenuItem>
          <SidebarMenuItem>
            <SidebarMenuButton asChild className="h-9 rounded-[10px] px-2.5">
              <Link
                to="/"
                aria-label="New chat"
                {...dragNew}
                onClick={(e) => {
                  e.preventDefault()
                  openNewChat(navigate)
                }}
              >
                <SquarePen /> Chat
                <span className="ml-auto text-xs text-fg-tertiary">{label(newChat)}</span>
              </Link>
            </SidebarMenuButton>
            <NewChatOnMenu />
          </SidebarMenuItem>
          <Capability required={talkRequirements} hide><SidebarMenuItem className="group/popout-row">
            <SidebarMenuButton asChild isActive={section === 'talk'} className={cn('h-9 rounded-[10px] px-2.5', live && talking.session && 'pointer-fine:pr-8')}>
              <Link
                to="/"
                aria-label={live ? 'Talk (running)' : 'Talk'}
                onClick={(e) => {
                  e.preventDefault()
                  void talk.open(api, current).then(
                    (session) => {
                      void queryClient.invalidateQueries({ queryKey: queryKeys.sessions })
                      return navigate({ to: '/talk/$session', params: { session } })
                    },
                    (err: unknown) => toast.error(errorText(err)),
                  )
                }}
              >
                <AudioLines /> Talk
                {live && <span className="ml-auto size-2 shrink-0 rounded-full bg-success" aria-label="talk running" />}
              </Link>
            </SidebarMenuButton>
            <SidebarPopoutButton target={live && talking.session ? { kind: 'talk', session: talking.session } : null} className="top-2 right-2" />
          </SidebarMenuItem></Capability>
          <Capability required={noteRequirements} hide><SidebarMenuItem>
            <SidebarMenuButton asChild className="h-9 rounded-[10px] px-2.5">
              <button type="button" aria-label="New note" disabled={createNoteAndOpen.isPending} onClick={() => createNoteAndOpen.mutate()}>
                {/* Label in a span (see the Meeting row); the plus overhangs the icon wrapper like the Meeting dot. */}
                <span className="relative inline-flex">
                  <StickyNote className="size-4" />
                  <Plus className={cn('absolute -top-1.5 -right-1.5 size-3', NOTE_PLUS)} strokeWidth={3.5} aria-hidden />
                </span>
                <span>Note</span>
              </button>
            </SidebarMenuButton>
          </SidebarMenuItem></Capability>
          {canRecord && (
            <SidebarMenuItem>
              <SidebarMenuButton asChild isActive={section === 'meeting'} className="h-9 rounded-[10px] px-2.5">
                <Link to="/meetings/new">
                  {/* The label is a span so the menu button's `[&>span:last-child]:truncate` (overflow hidden) hits it, not the icon wrapper whose dot overhangs. */}
                  <span className="relative inline-flex">
                    <Mic className="size-4" />
                    <span className="absolute -top-0.5 -right-0.5 size-1.5 rounded-full bg-danger" aria-hidden />
                  </span>
                  <span>Meeting</span>
                </Link>
              </SidebarMenuButton>
            </SidebarMenuItem>
          )}
          <Capability required={attentionRequirements} hide><SidebarMenuItem>
            <SidebarMenuButton asChild isActive={section === 'attention'} className="h-9 rounded-[10px] px-2.5">
              <Link to="/attention">
                <Bell /> Attention
                <AttentionBadge />
              </Link>
            </SidebarMenuButton>
          </SidebarMenuItem></Capability>
          {agentsTab && (<SidebarMenuItem>
            <SidebarMenuButton asChild isActive={section === 'agents'} className="h-9 rounded-[10px] px-2.5">
              <Link to={agentsTab.to}>
                <Bot /> Agents
              </Link>
            </SidebarMenuButton>
          </SidebarMenuItem>)}
          {/* Artifacts first (Sky, 2026-09-27): the most-opened Library entry; `/library` redirects there too. */}
          {libraryTab && (<SidebarMenuItem>
            <SidebarMenuButton asChild isActive={section === 'library'} className="h-9 rounded-[10px] px-2.5">
              <Link to={libraryTab.to}>
                <Library /> Library
              </Link>
            </SidebarMenuButton>
          </SidebarMenuItem>)}
          {/* Sky, 2026-11-02: the costs dashboard (160) gets its own row; the HUD SPEND button stays too. */}
          <Capability required={costsRequirements} hide><SidebarMenuItem>
            <SidebarMenuButton asChild isActive={section === 'costs'} className="h-9 rounded-[10px] px-2.5">
              <Link to="/costs" preload="intent">
                <ChartColumn /> Costs
              </Link>
            </SidebarMenuButton>
          </SidebarMenuItem></Capability>
          <Capability required={{ methods: ['desk_open', 'list_sessions'] }} hide><Desks active={session} /></Capability>
        </SidebarMenu>
        <SessionList scrollEl={scrollEl} barEl={barEl} active={session} />
      </SidebarContent>
      <div ref={setBarEl} className="shrink-0 empty:hidden" />
      <SidebarFooter className="px-1.5">
        <SidebarMenu>
          <SidebarMenuItem>
            {/* 188: Settings is a page; a phone opens its category list first. */}
            <SidebarMenuButton
              isActive={section === 'settings'}
              className="h-9 rounded-[10px] px-2.5"
              onClick={() => void (isMobile ? navigate({ to: '/settings' }) : settings(navigate, 'appearance'))}
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
    <Capability required={broadcastRequirements} hide><BroadcastDialog /></Capability>
    </>
  )
}