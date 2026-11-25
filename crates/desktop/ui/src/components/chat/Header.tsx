import { Capability } from '@/rpc/capabilities'
import { supports, type Requirements } from '@/rpc/discovery'
import { memoryRequirements, sessionCostRequirements, messagingRequirements, toolsRequirements, browserRequirements, approvalsRequirements, terminalRequirements, sessionMenuRequirements } from './requirements'
import { requirements as codemapRequirements } from '@/components/codemap/requirements'
import { requirements as artifactRequirements } from '@/components/artifacts/requirements'
import { searchRequirements } from '@/components/sidebar/requirements'

import { useQuerySupport } from '@/rpc/capabilities'
import { useQuery } from '@tanstack/react-query'
import { Link, useNavigate } from '@tanstack/react-router'
import { AppWindow, Contact, CornerLeftUp, Eye, MessageSquare, Network, Search, X } from 'lucide-react'
import { Fragment, type ReactNode, type RefObject, useEffect, useLayoutEffect, useRef, useState } from 'react'
import { flushSync } from 'react-dom'
import { SearchTrigger } from '@/components/sidebar/SearchTrigger'
import { EngineChip } from '@/components/sidebar/SessionItem'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip'
import { useSidebar } from '@/components/ui/sidebar'
import { cornerPad, headerToggle } from '@/lib/sidebar-corner'
import { SidebarToggle } from './SidebarToggle'
import { useTileHost } from '@/components/tiles/host'
import { openBrowserTile, openMemoryTile } from '@/components/tiles/open'
import { useViewBadge, useViewTitle } from '@danfessler/trellis-react'
import { useViewDrag } from '@/hooks/use-view-drag'
import { chatName } from '@/lib/attach'
import { sharedLabel, useSharing } from '@/lib/contacts'
import { chatTitle, popOutAndClose } from '@/components/tiles/TileFrame'
import { type HeaderId, type Mode, decideTier, headerOrder, layout } from '@/lib/header'
import { isMacDesktop } from '@/lib/hotkeys'
import { canPopout, isPopout, openPopout, type PopoutTarget } from '@/lib/popout'
import { ownerState, useEngineLabel, useOwnerOffline } from '@/lib/owner'
import { useMesh } from '@/components/tasks/queries'
import { queryKeys, useApi } from '@/lib/rpc'
import { cn } from '@/lib/utils'
import { closeTile, tileId, useTileOpen } from '@/lib/tiles'
import { sourceTag } from '@/lib/session-tree'
import type { ObservedMeta } from "@/rpc/bindings/method.agent.output"
import { useApprovals } from '@/store/approvals'
import { useConnection } from '@/store/connection'
import { useSessionField, useSessions } from '@/store/sessions'
import { ApprovalsSheet } from './ApprovalCard'
import { ArtifactPanel } from './ArtifactPanel'
import { CodeMapSheet } from '@/components/codemap/CodeMapHost'
import { BrowserLive } from './BrowserLive'
import { CostDialog } from './CostPill'
import { HeaderMenu, MenuRow } from './HeaderMenu'
import { DraftMemory, MemoryBadge, MemoryPanel } from './MemoryPanel'
import { SessionMenu } from './SessionMenu'
import { MessagingSheet } from './MessagingSheet'
import { ToolsSheet } from './ToolsSheet'
import { TerminalViewer } from './TerminalViewer'
import { EyeMark } from './ObservedBar'
import { surfaceOf } from './slash'
// 215 C3: no `model` item — the picker lives in the chat input (`ComposerModel`).
// The panels (terminals, browser, artifacts, CodeMap, a sandbox's desktop) live on the bar above the composer (`TrayItems`).
const HEADER_REQUIREMENTS: Partial<Record<HeaderId, Requirements>> = {
  search: searchRequirements, memory: memoryRequirements,
  pending: approvalsRequirements, menu: sessionMenuRequirements,
}

/** A subagent's way to its parent: an up-arrow icon button ("Open parent: <title>"), so the subagent's own title keeps
 * the room (a phone squeezed it to "Fea…" beside `↳ <parent title>`); `row`: the compact menu's "Parent" line. A
 * watch-only child (141 §9.2d) shows the `Watching` chip and `inside <parent> · <source>` instead. */
export function Parent({ id, mode, observed }: { id: string; mode: Mode; observed?: ObservedMeta | null }) {
  const canQuery1 = useQuerySupport(["list_sessions"])
  const row = mode === 'row'
  const api = useApi()
  const navigate = useNavigate()
  const { data: sessions } = useQuery({ enabled: canQuery1,
    queryKey: queryKeys.sessions,
    queryFn: () => api.list_sessions(),
  })
  const r = sessions?.find((s) => s.id === id)
  const name = r ? (r.name ?? chatName(r)) : 'Untitled chat'
  if (row) {
    return (
      <MenuRow
        icon={observed ? Eye : CornerLeftUp}
        label={observed ? 'Watching inside' : 'Parent'}
        detail={observed ? `${name} · ${sourceTag(observed)}` : name}
        onSelect={() => void navigate({ to: '/c/$session', params: { session: id } })}
      />
    )
  }
  if (observed) {
    return (
      <>
        <Badge variant="outline" className="shrink-0 gap-1 border-fg-accent/45 text-fg-accent" title={`watching · inside ${name} · ${sourceTag(observed)}`}>
          <Eye aria-hidden /> Watching
        </Badge>
        {mode === 'full' && (
          <Link to="/c/$session" params={{ session: id }} className="truncate text-xs text-fg-secondary hover:text-foreground">
            inside {name} · {sourceTag(observed)}
          </Link>
        )}
      </>
    )
  }
  const label = `Open parent: ${name}`
  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <Button
          variant="ghost"
          size="icon"
          className="shrink-0 rounded-lg text-fg-secondary"
          aria-label={label}
          onClick={() => void navigate({ to: '/c/$session', params: { session: id } })}
        >
          <CornerLeftUp className="size-5" />
        </Button>
      </TooltipTrigger>
      <TooltipContent>{label}</TooltipContent>
    </Tooltip>
  )
}

/** The header's `parent` item: a subagent's way up (`Parent`); nothing for a top-level chat. */
export function ParentItem({ parent, mode, observed }: { parent: string | null | undefined; mode: Mode; observed?: ObservedMeta | null }) {
  return parent ? <Parent id={parent} mode={mode} observed={observed} /> : null
}

/** `name · epoch N` for the compact menu's Owner line; 183 §12.1 (the phone): `name · live · epoch N`. */
function OwnerDetail({ owner, epoch, state }: { owner: string; epoch: number; state?: 'live' | 'connected' | 'offline' }) {
  const label = useEngineLabel(owner)
  return <span title={`owned by ${owner}`}>{[label, state === 'connected' ? undefined : state, `epoch ${epoch}`].filter(Boolean).join(' · ')}</span>
}

/**
 * The layout tier (lib/header.ts) for the pane this header sits in, from measurement rather than a viewport
 * breakpoint: `row` has `min-w-max`, so its width is the bar's natural width whenever that exceeds the pane's, and
 * `header.clientWidth` is the pane's. Re-checked after every render (a pill appeared) and on every pane resize;
 * both run before paint (layout effect; `flushSync` in the observer), so a wrong tier is never drawn.
 */
function useTier(header: RefObject<HTMLElement | null>, row: RefObject<HTMLElement | null>): number {
  const [tier, setTier] = useState(0)
  const need = useRef<(number | undefined)[]>([])
  const check = useRef(() => {})
  check.current = () => {
    const el = header.current
    const r = row.current
    if (!el || !r) return
    need.current[tier] = r.offsetWidth
    const next = decideTier(tier, el.clientWidth, need.current)
    if (next !== tier) setTier(next)
  }
  useLayoutEffect(() => check.current())
  useLayoutEffect(() => {
    const el = header.current
    if (!el) return
    const ro = new ResizeObserver(() => flushSync(() => check.current()))
    ro.observe(el)
    return () => ro.disconnect()
  }, [header])
  return tier
}

/**
 * 52 px bar over the column. Every control is one `HeaderItem` (`headerOrder`), rendered inline by the full bar and,
 * when the pane is too narrow for that (`useTier`), either kept in the compact bar (title · popout · chevron · ⋯) or
 * listed as a labelled row behind the chevron (`HeaderMenu`), so the two layouts share one source of truth.
 * The dialogs and sheets the items open live here, not in the items, so a menu that closes on selection cannot
 * unmount them; that includes the panels opened from the bar above the composer (`TrayItems`: the terminal viewer, the
 * live browser, the artifact panel, the CodeMap sheet). Draggable window region. `first` (default): this column touches
 * the window's top-left, so it carries the sidebar toggle and the traffic-light inset; `onClose` adds a close control
 * for a split pane.
 */
export function Header({
  session,
  first = true,
  onClose,
  popout,
}: {
  session: string | null
  first?: boolean
  onClose?(): void
  /** What "Open in new window" opens; the session's chat unless the host is another view of it (76: `talk`) or there is
   * no session yet (a draft: a New chat window). */
  popout?: PopoutTarget
}) {
  const discovery = useConnection(s => s.discovery)
  const canQuery2 = useQuerySupport(["list_sessions"])
  const api = useApi()
  const sidebar = useSidebar()
  const { isMobile } = sidebar
  const pending = useApprovals((s) => s.pending.length)
  const parent = useSessionField(session ?? '', 'parent')
  const observed = useSessionField(session ?? '', 'observed')
  const live = useSessionField(session ?? '', 'browserLive')
  const liveTitle = useSessionField(session ?? '', 'title')
  const running = useSessionField(session ?? '', 'running') === true
  // `list_sessions` rows: the title until `TitleChanged` lands, and the owner chip (68 §5) — patched live by `useInvalidateOn`.
  const sessions = useQuery({ queryKey: queryKeys.sessions, queryFn: () => api.list_sessions(), enabled: canQuery2 && (!!session) })
  const row = session ? sessions.data?.find((s) => s.id === session) : undefined
  // 183 §12.1: a remote chat's chip is live only while the follow stream is up (`FollowState`), offline when the owner is
  // not connected (or a routed call said so).
  const mesh = useMesh()
  const parked = useOwnerOffline((s) => !!session && s.by[session] !== undefined)
  const followed = useSessionField(session ?? '', 'following') === true
  const chip = row && !row.here ? ownerState(row.owner, mesh.data?.peers, parked, followed) : undefined
  // 183 §12.10: a chat exported to contacts says who can message it.
  const shared = sharedLabel(useSharing(session))
  const [sheet, setSheet] = useState(false)
  const [cost, setCost] = useState(false)
  const [terminal, setTerminal] = useState(false)
  const [memory, setMemory] = useState(false)
  // Inside a pane on the tiling layout, surfaces open as tiles beside this one; otherwise their sheets and dialogs.
  const host = useTileHost()
  const memoryTile = useTileOpen('memory', host ?? '')
  const toggleMemory = () =>
    host === null ? setMemory((o) => !o) : memoryTile ? closeTile(tileId('memory', host)) : void openMemoryTile(host)
  const mac = isMacDesktop()
  const ref = useRef<HTMLElement>(null)
  const rowRef = useRef<HTMLDivElement>(null)
  const tier = useTier(ref, rowRef)
  const compact = tier > 0

  const raw = session ? liveTitle || (row ? chatName(row) : 'Untitled chat') : 'New chat'
  const title = chatTitle(raw)
  // Beside other panes the bar drags this one (`useViewDrag`) and must not be a window drag region.
  const drag = useViewDrag()
  // The pane's Trellis tab (shown while it shares a panel): the chat's title, a dot while its turn runs.
  useViewTitle(title)
  useViewBadge(running || null)
  const region = (mac && drag.region) || undefined
  // A popped window has no sidebar to open and is already its own window: no toggle, no search, no popout button.
  const popped = isPopout()
  const nav = first && headerToggle(sidebar) && !popped
  /** The talk view (B3): the header carries the way back to the same session's chat; the talk keeps running. */
  const chat = popout?.kind === 'talk'
  const { bar, menu } = layout(headerOrder(canPopout() && !popped, chat, !!observed).filter(item => !HEADER_REQUIREMENTS[item.id] || supports(discovery, HEADER_REQUIREMENTS[item.id]!)), tier)

  const render: Record<HeaderId, (mode: Mode) => ReactNode> = {
    search: (mode) =>
      !nav ? null : mode === 'row' ? (
        <MenuRow icon={Search} label="Search" onSelect={() => window.dispatchEvent(new Event('theuth:search'))} />
      ) : (
        <SearchTrigger className="shrink-0" />
      ),
    sidebar: () => (nav ? <SidebarToggle /> : null),
    chat: () =>
      chat && session ? (
        <Button asChild variant="ghost" size={isMobile || compact ? 'icon' : 'sm'} className="shrink-0 rounded-lg text-fg-secondary">
          <Link to="/c/$session" params={{ session }} aria-label="Back to chat">
            <MessageSquare className="size-5" />
            {!isMobile && !compact && 'Back to chat'}
          </Link>
        </Button>
      ) : null,
    // `w-0 flex-1`: takes the slack without counting towards the bar's natural width, so a long title never pushes
    // the controls out; it truncates to nothing before they move (or, in the full bar, before the tier drops).
    title: () => (
      <span data-tauri-drag-region={region} className="flex w-0 min-w-0 flex-1 items-center gap-1.5 truncate text-sm font-medium" title={title}>
        {observed && <EyeMark className="size-3.5" />}
        <span className="truncate">{title}</span>
      </span>
    ),
    memory: (mode) => (session ? <MemoryBadge session={session} summary={row} row={mode === 'row'} open={host === null ? memory : memoryTile} onToggle={toggleMemory} /> : null),
    parent: (mode) => <ParentItem parent={parent} mode={mode} observed={observed} />,
    owner: (mode) =>
      row && !row.here ? (
        mode === 'row' ? (
          <MenuRow icon={Network} label="Owner" detail={<OwnerDetail owner={row.owner} epoch={row.epoch} state={chip} />} />
        ) : (
          <EngineChip owner={row.owner} epoch={row.epoch} prefix="on " state={chip} />
        )
      ) : null,
    shared: (mode) =>
      !shared ? null : mode === 'row' ? (
        <MenuRow icon={Contact} label="Shared" detail={shared.replace(/^Shared with /, '')} />
      ) : (
        <Badge variant="outline" className="max-w-64 justify-start text-fg-secondary" title={shared}>
          <Contact aria-hidden="true" />
          <span className="truncate">{shared}</span>
        </Badge>
      ),
    pending: (mode) =>
      pending === 0 ? null : mode === 'row' ? (
        <MenuRow label="Pending approvals" detail={pending} className="text-warning" onSelect={() => setSheet(true)} />
      ) : (
        <Button variant="ghost" className="h-7 rounded-full px-2 text-warning" onClick={() => setSheet(true)} aria-label="Pending approvals">
          ●{pending}
        </Button>
      ),
    more: (mode) =>
      mode === 'full' ? null : (
        <HeaderMenu header={ref} badge={pending > 0}>
          {menu.map((i) => (
            <Fragment key={i.id}>{render[i.id]('row')}</Fragment>
          ))}
        </HeaderMenu>
      ),
    popout: (mode) => {
      const target = popout ?? (session ? ({ kind: 'chat', session } as const) : null)
      if (!target) return null
      // A chat pane moved to its own window leaves this layout by its own Close (`onClose`, present only beside other
      // chat panes): the pane alone goes, the session is untouched. A lone pane, a draft or a talk stays.
      const pop = () => void (target.kind === 'chat' && onClose ? popOutAndClose(() => openPopout(target), onClose) : openPopout(target).catch((e: unknown) => console.warn('open_window', e)))
      return mode === 'row' ? (
        <MenuRow icon={AppWindow} label="Open in new window" onSelect={pop} />
      ) : (
        <Button variant="ghost" className="size-9 rounded-lg text-fg-secondary" aria-label="Open in new window" onClick={pop}>
          <AppWindow className="size-5" />
        </Button>
      )
    },
    menu: () => <SessionMenu session={session ?? ''} />,
    close: () =>
      onClose ? (
        <Button variant="ghost" size="icon" className="rounded-lg text-fg-secondary" aria-label="Close pane" onClick={onClose}>
          <X className="size-5" />
        </Button>
      ) : null,
  }

  useEffect(() => {
    if (session) return
    const on = (e: Event) => {
      if (surfaceOf(e) !== 'memory') return
      if (host === null) setMemory(true)
      else void openMemoryTile(host)
    }
    window.addEventListener('theuth:open', on)
    return () => window.removeEventListener('theuth:open', on)
  }, [session, host])
  // `BrowserShowLive` sets `browserLive` (the reducer): where this chat tiles, that is the live browser tile instead.
  // The one owner of that conversion; a tile that could not open leaves the request for the dialog.
  useEffect(() => {
    if (!live || !session || host === null) return
    if (openBrowserTile(host) !== null) useSessions.getState().setBrowserLive(session, false)
  }, [live, session, host])
  return (
    <header ref={ref} {...drag.props} data-header-tier={tier} className="relative flex h-[52px] shrink-0 overflow-hidden">
      <div
        ref={rowRef}
        data-tauri-drag-region={region}
        className={cn('flex h-full w-full min-w-max items-center', compact ? 'gap-1 px-2' : 'gap-2 px-3', first && !popped && mac && cornerPad(sidebar))}
      >
        {bar.map((i) => (
          <Fragment key={i.id}>{render[i.id](compact ? 'bar' : 'full')}</Fragment>
        ))}
      </div>
      <Capability required={approvalsRequirements} hide><ApprovalsSheet open={sheet} onOpenChange={setSheet} /></Capability>
      {!session && <DraftMemory open={memory} onOpenChange={setMemory} />}
      <Capability required={messagingRequirements} hide><MessagingSheet session={session ?? ''} /></Capability>
      <Capability required={toolsRequirements} hide><ToolsSheet session={session ?? ''} /></Capability>
      {session && (
        <>
          <Capability required={sessionCostRequirements} hide><CostDialog session={session} open={cost} onOpenChange={setCost} /></Capability>
          <Capability required={terminalRequirements} hide><TerminalViewer session={session} open={terminal} onOpenChange={setTerminal} /></Capability>
          <Capability required={browserRequirements} hide><BrowserLive session={session} open={!!live && host === null} onOpenChange={(o) => useSessions.getState().setBrowserLive(session, o)} /></Capability>
          <Capability required={artifactRequirements} hide><ArtifactPanel session={session} /></Capability>
          <Capability required={codemapRequirements} hide><CodeMapSheet session={session} /></Capability>
          <Capability required={memoryRequirements} hide><MemoryPanel session={session} open={memory} onOpenChange={setMemory} /></Capability>
        </>
      )}
    </header>
  )
}
