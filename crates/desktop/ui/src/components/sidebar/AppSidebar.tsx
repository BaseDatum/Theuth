import { Capability } from '@/rpc/capabilities'
import { supports } from '@/rpc/discovery'
import { useConnection } from '@/store/connection'
import { GROUPS } from '@/lib/nav'
import { requirements as talkRequirements } from '@/components/talk/requirements'
import { requirements as noteRequirements } from '@/components/notes/requirements'
import { requirements as whiteboardRequirements } from '@/components/whiteboard/requirements'
import { requirements as deckRequirements } from '@/components/decks/requirements'
import { requirements as attentionRequirements } from '@/components/attention/requirements'
import { requirements as costsRequirements } from '@/components/costs/requirements'
import { broadcastRequirements } from '@/components/chat/requirements'
import { useMutation, useQueryClient } from '@tanstack/react-query'
import { Link, useNavigate, useRouterState } from '@tanstack/react-router'
import { AudioLines, Bot, ChartColumn, FolderKanban, Inbox, Library, Megaphone, PanelLeft, Plus, Settings, SquarePen, StickyNote, Users } from 'lucide-react'
import { type ReactNode, useEffect, useState } from 'react'
import { notify as toast } from '@/lib/notify'
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
import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip'
import { BroadcastDialog } from '@/components/chat/BroadcastDialog'
import { openBroadcastDialog } from '@/components/chat/broadcast'
import { NewChatOnMenu, machineRows, openNewChat } from '@/components/chat/NewChatPane'
import { useMesh } from '@/components/tasks/queries'
import { createNote } from '@/components/notes/create'
import { useSessionDrag, useViewDrag } from '@/components/chat/Panes'
import { AttentionBadge } from '@/components/hud/AttentionBadge'
import { useCanRecord } from '@/components/meeting/MeetingList'
import { settings } from '@/components/chat/slash'
import { errorText } from '@/components/settings/shared'
import { Checklist } from '@/components/onboarding/Checklist'
import { SidebarSlot } from '@/components/plugins/SidebarSlot'
import { StatusStrip } from '@/components/plugins/StatusStrip'
import { isMacDesktop } from '@/lib/hotkeys'
import { sectionOf } from '@/lib/nav'
import { canPopout, noteOpenMode, openPopout } from '@/lib/popout'
import { label } from '@/lib/chord'
import { queryKeys, useApi } from '@/lib/rpc'
import { useShortcuts } from '@/lib/shortcuts'
import * as talk from '@/lib/talk'
import { useUpdate } from '@/lib/update'
import { takeReveal, useSidebarReveal } from '@/lib/sidebar-reveal'
import { cn } from '@/lib/utils'
import type { DocSummary } from "@/rpc/bindings/method.agent.output"
import { Desks } from './Desks'
import { ICON_COUNT, ICON_MARK, ICON_PLUS, RAIL_ITEM, RailSections, useSettledRail } from './Rail'
import { SearchTrigger } from './SearchTrigger'
import { SessionList } from './SessionList'
import { SidebarPopoutButton } from './SidebarPopoutButton'
import { WhiteboardsRail, WhiteboardsRow } from './Whiteboards'
import { DecksRail, DecksRow } from './Decks'

/** Injected by vite `define` from tauri.conf.json. */
declare const __APP_VERSION__: string

/** 143 D2: the plus on the Note row — malachite (`--status-success-fg`); one token to swap for `text-project-nile` (olive). */
export const NOTE_PLUS = 'text-success'

/** A rail icon's name, to its right on hover (the rail shows no labels). */
function RailTip({ label, children }: { label: string; children: ReactNode }) {
  return (
    <Tooltip>
      <TooltipTrigger asChild>{children}</TooltipTrigger>
      <TooltipContent side="right">{label}</TooltipContent>
    </Tooltip>
  )
}

/**
 * ChatGPT's left rail: Chat, Inbox, Talk, Note, Meeting, Agents, Library, Costs, the desks, the chat list, Settings; Search sits in
 * the header beside the toggle (49 §5.1). 97: the five Agents pages and the five Library pages are one row each; the
 * page under it carries the tab band (`SectionBand`), so the rail stays short.
 */
export function AppSidebar({ session }: { session: string | undefined }) {
  const discovery = useConnection(s => s.discovery)
  const agentsTab = GROUPS.agents.find(tab => supports(discovery, tab.required))
  const libraryTab = GROUPS.library.find(tab => supports(discovery, tab.required))
  // 223: Projects, or the task board alone on an engine without the project methods.
  const projectsTab = GROUPS.projects.find(tab => supports(discovery, tab.required))
  const navigate = useNavigate()
  const { isMobile, setOpenMobile, toggleSidebar, state } = useSidebar()
  // Desktop collapsed: the icon rail (a phone keeps the full Sheet).
  const rail = !isMobile && state === 'collapsed'
  // The expanded sidebar, once shown, stays mounted behind the rail (hidden, inert, at its own fixed width), so a toggle
  // never tears down and rebuilds the chat list (its virtualizer and measurements), the desks or the plugin slot.
  const [kept, setKept] = useState(!rail)
  if (!rail && !kept) setKept(true)
  const mac = isMacDesktop()
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
  // The rail's Desks "Show all →": the expanded sidebar scrolls the Desks group into view (the list's sections are `SessionList`'s).
  const reveal = useSidebarReveal((s) => s.key)
  useEffect(() => {
    if (reveal !== 'desks' || !scrollEl) return
    scrollEl.querySelector('[data-desks]')?.scrollIntoView({ block: 'start' })
    takeReveal()
  }, [reveal, scrollEl])
  // Slot for `SessionList`'s multi-select bar: a sibling below the scroller, so the bar stays put while the list scrolls.
  const [barEl, setBarEl] = useState<HTMLDivElement | null>(null)
  // The rail's scrolling middle, settled once the collapse animation ends (iPadOS left it blank; `settleRail`).
  const [railNav, setRailNav] = useState<HTMLElement | null>(null)
  useSettledRail(railNav)
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
  // focused pane when the view is split, else goes to `/new`.
  const dragNew = useSessionDrag(null, 'New chat')
  const dragInbox = useViewDrag('/attention', 'Inbox')
  const dragAgents = useViewDrag(agentsTab?.to ?? '/agents', 'Agents')
  const dragLibrary = useViewDrag(libraryTab?.to ?? '/artifacts', 'Library')
  const dragProjects = useViewDrag(projectsTab?.to ?? '/projects', 'Projects')
  const dragCosts = useViewDrag('/costs', 'Costs')
  const dragMeeting = useViewDrag('/meetings/new', 'Meeting')
  const dragSettings = useViewDrag('/settings/appearance', 'Settings')
  // The Chat row's ▸ ("New chat on", only with peers) sits at the right edge; the popout button moves left of it.
  const peers = machineRows(useMesh().data).length > 0
  // Every destination row's "Open in new window" (`SidebarPopoutButton`): the row's own click, in its own window. The
  // padding is reserved whenever the button can exist, so hovering never shifts the row.
  const pad = canPopout() && 'pointer-fine:pr-8'
  // 143 D3: Note creates then opens — its own window on a desktop (the shell stays put), the note page on a phone/iOS
  // (the sheet closes on the href change above). Reads the notes cache rather than `useNotes()`: no permanent subscription.
  // `popout` (the row's popout button): the same create, then always its own window.
  const createNoteAndOpen = useMutation({
    mutationFn: (_popout: boolean) => createNote(api, queryClient, queryClient.getQueryData<DocSummary[]>(queryKeys.notes) ?? []),
    onSuccess: (n, popout) => {
      if (popout || noteOpenMode(isMobile, canPopout()) === 'popout') void openPopout({ kind: 'note', doc: n.id, fresh: true })
      else void navigate({ to: '/notes/$id', params: { id: n.id }, search: { new: true } })
    },
    onError: (e) => toast.error(errorText(e)),
  })
  // The Talk row's click (and its popout button while idle): talk where the row says, then `show` the talk's session.
  const openTalk = (show: (session: string) => unknown) =>
    void talk.open(api, current).then(
      (session) => {
        void queryClient.invalidateQueries({ queryKey: queryKeys.sessions })
        return show(session)
      },
      (err: unknown) => toast.error(errorText(err)),
    )

  return (
    <>
    <Sidebar collapsible="icon" className="border-r-0">
      {/* The rail first, so a lookup in document order (a search trigger, a tour target) finds it over the hidden sidebar
       * behind it. It sits at the container's trailing edge: the only part a collapsed sidebar shows (`Sidebar`). */}
      {rail && (
        <div className="absolute inset-y-0 right-0 z-10 flex w-(--sidebar-width-icon) flex-col items-center bg-sidebar">
          {/* The macOS traffic lights sit over the rail's top: this strip is theirs (and drags the window); the 3.5rem
           * rail is narrower than they reach, so the header right of it adds the rest (`cornerPad`). */}
          {mac && <div data-tauri-drag-region className="h-[52px] w-full shrink-0" />}
          <div className={cn('flex shrink-0 flex-col items-center gap-1 pb-1', !mac && 'pt-2')}>
            <RailTip label="Expand sidebar">
              <button type="button" className={RAIL_ITEM} aria-label="Expand sidebar" onClick={toggleSidebar}>
                <PanelLeft />
              </button>
            </RailTip>
            <EngineSwitcher compact className="pointer-coarse:size-11" />
            <SearchTrigger side="right" className="pointer-coarse:size-11" />
            <Capability required={broadcastRequirements} hide>
              <RailTip label="Broadcast…">
                <button type="button" className={RAIL_ITEM} aria-label="Broadcast…" onClick={() => openBroadcastDialog({})}>
                  <Megaphone />
                </button>
              </RailTip>
            </Capability>
          </div>
          <nav ref={setRailNav} aria-label="Sidebar" className="flex min-h-0 w-full flex-1 flex-col items-center gap-1 overflow-x-hidden overflow-y-auto py-1 [scrollbar-width:none]">
            <RailTip label={`New chat${newChat ? ` (${label(newChat)})` : ''}`}>
              <Link
                to="/new"
                aria-label="New chat"
                className={RAIL_ITEM}
                {...dragNew}
                onClick={(e) => {
                  e.preventDefault()
                  openNewChat(navigate)
                }}
              >
                <SquarePen />
              </Link>
            </RailTip>
            <Capability required={attentionRequirements} hide>
              <RailTip label="Inbox">
                <Link to="/attention" aria-label="Inbox" className={RAIL_ITEM} data-active={section === 'attention' || undefined} data-tour="nav-inbox" {...dragInbox}>
                  <span className="relative inline-flex">
                    <Inbox />
                    <AttentionBadge className={ICON_COUNT} />
                  </span>
                </Link>
              </RailTip>
            </Capability>
            <Capability required={talkRequirements} hide>
              <RailTip label={live ? 'Talk (running)' : 'Talk'}>
                <button
                  type="button"
                  aria-label={live ? 'Talk (running)' : 'Talk'}
                  className={RAIL_ITEM}
                  data-active={section === 'talk' || undefined}
                  onClick={() => openTalk((session) => navigate({ to: '/talk/$session', params: { session } }))}
                >
                  <span className="relative inline-flex">
                    <AudioLines />
                    {live && <span className={cn(ICON_MARK, 'bg-success')} aria-label="talk running" />}
                  </span>
                </button>
              </RailTip>
            </Capability>
            <Capability required={noteRequirements} hide>
              <RailTip label="New note">
                <button type="button" aria-label="New note" className={RAIL_ITEM} disabled={createNoteAndOpen.isPending} onClick={() => createNoteAndOpen.mutate(false)}>
                  <span className="relative inline-flex">
                    <StickyNote className="size-[18px]" />
                    <Plus className={cn(ICON_PLUS, NOTE_PLUS)} strokeWidth={3.5} aria-hidden />
                  </span>
                </button>
              </RailTip>
            </Capability>
            <Capability required={whiteboardRequirements} hide>
              <WhiteboardsRail itemClass={RAIL_ITEM} wrap={(t) => <RailTip label="Whiteboards">{t}</RailTip>} />
            </Capability>
            <Capability required={deckRequirements} hide>
              <DecksRail itemClass={RAIL_ITEM} wrap={(t) => <RailTip label="Decks">{t}</RailTip>} />
            </Capability>
            {canRecord && (
              <RailTip label="Meeting">
                <Link to="/meetings/new" aria-label="Meeting" className={RAIL_ITEM} data-active={section === 'meeting' || undefined} {...dragMeeting}>
                  <span className="relative inline-flex">
                    <Users className="size-[18px]" />
                    <span className={cn(ICON_MARK, 'bg-danger')} aria-hidden />
                  </span>
                </Link>
              </RailTip>
            )}
            {agentsTab && (
              <RailTip label="Agents">
                <Link to={agentsTab.to} aria-label="Agents" className={RAIL_ITEM} data-active={section === 'agents' || undefined} data-tour="nav-agents" {...dragAgents}>
                  <Bot />
                </Link>
              </RailTip>
            )}
            {projectsTab && (
              <RailTip label="Projects">
                <Link to={projectsTab.to} aria-label="Projects" className={RAIL_ITEM} data-active={section === 'projects' || undefined} data-tour="nav-projects" {...dragProjects}>
                  <FolderKanban />
                </Link>
              </RailTip>
            )}
            {libraryTab && (
              <RailTip label="Library">
                <Link to={libraryTab.to} aria-label="Library" className={RAIL_ITEM} data-active={section === 'library' || undefined} data-tour="nav-library" {...dragLibrary}>
                  <Library />
                </Link>
              </RailTip>
            )}
            <Capability required={costsRequirements} hide>
              <RailTip label="Costs">
                <Link to="/costs" preload="intent" aria-label="Costs" className={RAIL_ITEM} data-active={section === 'costs' || undefined} data-tour="nav-costs" {...dragCosts}>
                  <ChartColumn />
                </Link>
              </RailTip>
            </Capability>
            <div role="separator" className="my-1.5 h-px w-8 shrink-0 bg-sidebar-border" />
            <RailSections active={session} />
            <SidebarSlot variant="rail" />
          </nav>
          <div className="flex shrink-0 flex-col items-center py-2">
            <RailTip label={`Settings · v${__APP_VERSION__}`}>
              <button
                type="button"
                aria-label="Settings"
                className={RAIL_ITEM}
                data-active={section === 'settings' || undefined}
                data-tour="nav-settings"
                {...dragSettings}
                onClick={() => void settings(navigate, 'appearance')}
              >
                <Settings />
                {pending !== null && <span className="absolute top-1.5 right-1.5 size-1.5 rounded-full bg-fg-tertiary" aria-label="update ready" />}
              </button>
            </RailTip>
          </div>
        </div>
      )}
      {(kept || !rail) && (
      <div
        // Fades out over the collapse slide and is then `invisible`; layout is kept (no reflow, no resize observer fires).
        className={cn('flex min-h-0 w-full flex-1 flex-col transition-[opacity,visibility] duration-200', rail && 'invisible opacity-0')}
        inert={rail}
        aria-hidden={rail || undefined}
      >
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
            aria-label={isMobile ? 'Close sidebar' : 'Collapse sidebar'}
            onClick={toggleSidebar}
          >
            <PanelLeft className="size-5" />
          </Button>
        </div>
      </SidebarHeader>
      <SidebarContent ref={setScrollEl} className="gap-0 overflow-y-auto">
        <SidebarMenu className="px-1.5">
          <SidebarMenuItem className="group/popout-row">
            <SidebarMenuButton asChild className={cn('h-9 rounded-[10px] px-2.5', canPopout() && (peers ? 'pointer-fine:pr-14' : 'pointer-fine:pr-8'))}>
              <Link
                to="/new"
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
            {/* New chat creates nothing until its first send (`/new`), so its window is that page; every press a new
             * window (`key`), since the last one may already be the chat its first send made. */}
            <SidebarPopoutButton
              target={() => void openPopout({ kind: 'page', path: '/new', key: crypto.randomUUID().slice(0, 8) })}
              className={cn('top-2', peers ? 'right-8' : 'right-2')}
            />
            <NewChatOnMenu />
          </SidebarMenuItem>
          <Capability required={attentionRequirements} hide><SidebarMenuItem className="group/popout-row" data-tour="nav-inbox">
            <SidebarMenuButton asChild isActive={section === 'attention'} className={cn('h-9 rounded-[10px] px-2.5', pad)}>
              <Link to="/attention" {...dragInbox}>
                <Inbox /> Inbox
                <AttentionBadge />
              </Link>
            </SidebarMenuButton>
            <SidebarPopoutButton target={{ kind: 'page', path: '/attention' }} className="top-2 right-2" />
          </SidebarMenuItem></Capability>
          <Capability required={talkRequirements} hide><SidebarMenuItem className="group/popout-row">
            <SidebarMenuButton asChild isActive={section === 'talk'} className={cn('h-9 rounded-[10px] px-2.5', pad)}>
              <Link
                to="/"
                aria-label={live ? 'Talk (running)' : 'Talk'}
                onClick={(e) => {
                  e.preventDefault()
                  openTalk((session) => navigate({ to: '/talk/$session', params: { session } }))
                }}
              >
                <AudioLines /> Talk
                {live && <span className="ml-auto size-2 shrink-0 rounded-full bg-success" aria-label="talk running" />}
              </Link>
            </SidebarMenuButton>
            <SidebarPopoutButton
              target={live && talking.session ? { kind: 'talk', session: talking.session } : () => openTalk((session) => openPopout({ kind: 'talk', session }))}
              className="top-2 right-2"
            />
          </SidebarMenuItem></Capability>
          <Capability required={noteRequirements} hide><SidebarMenuItem className="group/popout-row">
            <SidebarMenuButton asChild className={cn('h-9 rounded-[10px] px-2.5', pad)}>
              <button type="button" aria-label="New note" disabled={createNoteAndOpen.isPending} onClick={() => createNoteAndOpen.mutate(false)}>
                {/* Label in a span (see the Meeting row); the plus sits on the icon wrapper, same corner as the Meeting dot. */}
                <span className="relative inline-flex">
                  <StickyNote className="size-4" />
                  <Plus className={cn(ICON_PLUS, NOTE_PLUS)} strokeWidth={3.5} aria-hidden />
                </span>
                <span>Note</span>
              </button>
            </SidebarMenuButton>
            <SidebarPopoutButton target={() => { if (!createNoteAndOpen.isPending) createNoteAndOpen.mutate(true) }} className="top-2 right-2" />
          </SidebarMenuItem></Capability>
          <Capability required={whiteboardRequirements} hide><WhiteboardsRow className="h-9 rounded-[10px] px-2.5" /></Capability>
          <Capability required={deckRequirements} hide><DecksRow className="h-9 rounded-[10px] px-2.5" /></Capability>
          {canRecord && (
            <SidebarMenuItem className="group/popout-row">
              <SidebarMenuButton asChild isActive={section === 'meeting'} className={cn('h-9 rounded-[10px] px-2.5', pad)}>
                <Link to="/meetings/new" {...dragMeeting}>
                  {/* The label is a span so the menu button's `[&>span:last-child]:truncate` (overflow hidden) hits it, not the icon wrapper whose dot overhangs. */}
                  <span className="relative inline-flex">
                    <Users className="size-4" />
                    <span className={cn(ICON_MARK, 'bg-danger')} aria-hidden />
                  </span>
                  <span>Meeting</span>
                </Link>
              </SidebarMenuButton>
              {/* The row opens the start form, so its window is the form; Start there starts the meeting, as in the shell. */}
              <SidebarPopoutButton target={{ kind: 'page', path: '/meetings/new' }} className="top-2 right-2" />
            </SidebarMenuItem>
          )}
          {agentsTab && (<SidebarMenuItem className="group/popout-row" data-tour="nav-agents">
            <SidebarMenuButton asChild isActive={section === 'agents'} className={cn('h-9 rounded-[10px] px-2.5', pad)}>
              <Link to={agentsTab.to} {...dragAgents}>
                <Bot /> Agents
              </Link>
            </SidebarMenuButton>
            <SidebarPopoutButton target={{ kind: 'page', path: agentsTab.to }} className="top-2 right-2" />
          </SidebarMenuItem>)}
          {projectsTab && (<SidebarMenuItem className="group/popout-row" data-tour="nav-projects">
            <SidebarMenuButton asChild isActive={section === 'projects'} className={cn('h-9 rounded-[10px] px-2.5', pad)}>
              <Link to={projectsTab.to} {...dragProjects}>
                <FolderKanban /> Projects
              </Link>
            </SidebarMenuButton>
            <SidebarPopoutButton target={{ kind: 'page', path: projectsTab.to }} className="top-2 right-2" />
          </SidebarMenuItem>)}
          {/* Artifacts first (Sky, 2026-09-27): the most-opened Library entry; `/library` redirects there too. */}
          {libraryTab && (<SidebarMenuItem className="group/popout-row" data-tour="nav-library">
            <SidebarMenuButton asChild isActive={section === 'library'} className={cn('h-9 rounded-[10px] px-2.5', pad)}>
              <Link to={libraryTab.to} {...dragLibrary}>
                <Library /> Library
              </Link>
            </SidebarMenuButton>
            <SidebarPopoutButton target={{ kind: 'page', path: libraryTab.to }} className="top-2 right-2" />
          </SidebarMenuItem>)}
          {/* Sky, 2026-11-02: the costs dashboard (160) gets its own row; the HUD SPEND button stays too. */}
          <Capability required={costsRequirements} hide><SidebarMenuItem className="group/popout-row" data-tour="nav-costs">
            <SidebarMenuButton asChild isActive={section === 'costs'} className={cn('h-9 rounded-[10px] px-2.5', pad)}>
              <Link to="/costs" preload="intent" {...dragCosts}>
                <ChartColumn /> Costs
              </Link>
            </SidebarMenuButton>
            <SidebarPopoutButton target={{ kind: 'page', path: '/costs' }} className="top-2 right-2" />
          </SidebarMenuItem></Capability>
          <Capability required={{ methods: ['desk_open', 'list_sessions'] }} hide><Desks active={session} /></Capability>
        </SidebarMenu>
        {/* 216: plugin icons between the desks and the chat list. */}
        <SidebarSlot variant="row" />
        <SessionList scrollEl={scrollEl} barEl={barEl} active={session} />
      </SidebarContent>
      <div ref={setBarEl} className="shrink-0 empty:hidden" />
      <SidebarFooter className="px-1.5">
        <Checklist />
        {/* 216: on a phone the status bar is gone; plugin status items live here. */}
        <StatusStrip variant="sheet" />
        <SidebarMenu>
          <SidebarMenuItem className="group/popout-row" data-tour="nav-settings">
            {/* 188: Settings is a page; a phone opens its category list first. */}
            <SidebarMenuButton
              isActive={section === 'settings'}
              className={cn('h-9 rounded-[10px] px-2.5', pad)}
              {...dragSettings}
              onClick={() => void (isMobile ? navigate({ to: '/settings' }) : settings(navigate, 'appearance'))}
            >
              <Settings /> Settings
              <span className="ml-auto text-xs text-fg-tertiary">v{__APP_VERSION__}</span>
              {pending !== null && <span className="size-1.5 rounded-full bg-fg-tertiary" aria-label="update ready" />}
            </SidebarMenuButton>
            <SidebarPopoutButton target={{ kind: 'page', path: isMobile ? '/settings' : '/settings/appearance' }} className="top-2 right-2" />
          </SidebarMenuItem>
        </SidebarMenu>
      </SidebarFooter>
      </div>
      )}
    </Sidebar>
    {/* Beside, not inside: on phones the sidebar is a sheet that unmounts when closed (62 §5.2). */}
    <Capability required={broadcastRequirements} hide><BroadcastDialog active={session} /></Capability>
    </>
  )
}