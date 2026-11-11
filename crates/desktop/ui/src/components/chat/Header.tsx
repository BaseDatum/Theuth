import { useQuery } from '@tanstack/react-query'
import { Link, useNavigate } from '@tanstack/react-router'
import { AppWindow, Contact, CornerLeftUp, Eye, Gauge, LayoutGrid, MessageSquare, Monitor, Network, PanelLeft, Search, ShieldOff, X } from 'lucide-react'
import { Fragment, type ReactNode, type RefObject, useLayoutEffect, useRef, useState } from 'react'
import { flushSync } from 'react-dom'
import { SearchTrigger } from '@/components/sidebar/SearchTrigger'
import { useSandboxes } from '@/components/settings/sandbox-queries'
import { EngineChip } from '@/components/sidebar/SessionItem'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuTrigger } from '@/components/ui/dropdown-menu'
import { useSidebar } from '@/components/ui/sidebar'
import { chatName } from '@/lib/attach'
import { sharedLabel, useSharing } from '@/lib/contacts'
import { previewText } from '@/lib/channel-origin'
import { type HeaderId, type Mode, approvalsOff as pillOff, decideTier, headerOrder, layout } from '@/lib/header'
import { isMacDesktop } from '@/lib/hotkeys'
import { canPopout, isPopout, openPopout, type PopoutTarget } from '@/lib/popout'
import { ownerState, useEngineLabel, useOwnerOffline } from '@/lib/owner'
import { useMesh } from '@/components/tasks/queries'
import { queryKeys, useApi } from '@/lib/rpc'
import { cn } from '@/lib/utils'
import { sourceTag } from '@/lib/session-tree'
import type { ObservedMeta } from '@/rpc/bindings/ObservedMeta'
import { parseSpoken } from '@/lib/injected'
import { parseNote } from '@/lib/voice-note'
import { useApprovals } from '@/store/approvals'
import { useConnection } from '@/store/connection'
import { useBrowserBusy, useSessionField, useSessions } from '@/store/sessions'
import { ApprovalsSheet } from './ApprovalCard'
import { metaLine } from './ArtifactCard'
import { ArtifactPanel } from './ArtifactPanel'
import { CodeMapButton, CodeMapSheet, useCodemapAvailable } from '@/components/codemap/CodeMapHost'
import { BrowserLive } from './BrowserLive'
import { ContextMeter } from './ContextMeter'
import { CostDialog, CostPill } from './CostPill'
import { HeaderMenu, MenuRow, useHeaderMenuClose } from './HeaderMenu'
import { ModelDialog, ModelMenu, ModelRow } from './ModelMenu'
import { MemoryBadge, MemoryPanel } from './MemoryPanel'
import { BrowserIndicator } from './Indicators'
import { SessionMenu } from './SessionMenu'
import { MessagingSheet } from './MessagingSheet'
import { TerminalButton, TerminalViewer, useHasTerminals } from './TerminalViewer'
import { EyeMark } from './ObservedBar'

/** `↳ parent` breadcrumb of a subagent's transcript; `row`: the compact menu's "Parent" line. A watch-only child (141 §9.2d)
 * shows the `Watching` chip and `inside <parent> · <source>` instead. */
function Parent({ id, mode, observed }: { id: string; mode: Mode; observed?: ObservedMeta | null }) {
  const row = mode === 'row'
  const api = useApi()
  const navigate = useNavigate()
  const { data: sessions } = useQuery({
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
  return (
    <Link
      to="/c/$session"
      params={{ session: id }}
      className="truncate text-sm text-fg-secondary hover:text-foreground"
    >
      ↳ {name}
    </Link>
  )
}

/** `name · epoch N` for the compact menu's Owner line; 183 §12.1 (the phone): `name · live · epoch N`. */
function OwnerDetail({ owner, epoch, state }: { owner: string; epoch: number; state?: 'live' | 'connected' | 'offline' }) {
  const label = useEngineLabel(owner)
  return <span title={`owned by ${owner}`}>{[label, state === 'connected' ? undefined : state, `epoch ${epoch}`].filter(Boolean).join(' · ')}</span>
}

/** `▣ N`: the session's artifacts, newest first; picking one opens the panel (35 §7.2). `row`: the compact menu's line. */
function Artifacts({ session, row }: { session: string; row?: boolean }) {
  const artifacts = useSessionField(session, 'artifacts')
  const close = useHeaderMenuClose()
  if (!artifacts?.size) return null
  const list = [...artifacts.values()].sort((x, y) => y.created_at - x.created_at)
  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        {row ? (
          <MenuRow icon={LayoutGrid} label="Artifacts" detail={list.length} />
        ) : (
          <Button variant="ghost" className="h-9 gap-1 rounded-lg px-2 text-fg-secondary" aria-label="Artifacts">
            ▣<span className="text-xs">{list.length}</span>
          </Button>
        )}
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end" className="w-72">
        {list.map((a) => (
          <DropdownMenuItem
            key={a.call_id}
            className="flex-col items-start gap-0"
            onSelect={() => {
              close()
              useSessions.getState().setArtifactPanel(session, a.call_id)
            }}
          >
            <span className="w-full truncate">{a.title}</span>
            <span className="w-full truncate text-xs text-fg-tertiary">
              {metaLine(a)}
              {a.version > 1 ? ` · v${a.version}` : ''}
            </span>
          </DropdownMenuItem>
        ))}
      </DropdownMenuContent>
    </DropdownMenu>
  )
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
 * when the pane is too narrow for that (`useTier`), either kept in the compact bar (title · context % · one panel
 * button · chevron · ⋯) or listed as a labelled row behind the chevron (`HeaderMenu`), so the two layouts share one
 * source of truth.
 * The dialogs and sheets the items open live here, not in the items, so a menu that closes on selection cannot
 * unmount them. Draggable window region. `first` (default): this column touches the window's top-left, so it carries
 * the sidebar toggle and the traffic-light inset; `onClose` adds a close control for a split pane.
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
  /** What "Open in new window" opens; the session's chat unless the host is another view of it (76: `talk`). */
  popout?: PopoutTarget
}) {
  const api = useApi()
  const { open, isMobile } = useSidebar()
  const info = useConnection((s) => s.info)
  const pending = useApprovals((s) => s.pending.length)
  const parent = useSessionField(session ?? '', 'parent')
  const observed = useSessionField(session ?? '', 'observed')
  const browser = useSessionField(session ?? '', 'browser')
  const browserBusy = useBrowserBusy(session ?? '')
  const live = useSessionField(session ?? '', 'browserLive')
  const artifacts = useSessionField(session ?? '', 'artifacts')
  const terminals = useHasTerminals(session)
  const codemap = useCodemapAvailable(session)
  const liveTitle = useSessionField(session ?? '', 'title')
  // `list_sessions` rows: the title until `TitleChanged` lands, and the owner chip (68 §5) — patched live by `useInvalidateOn`.
  const sessions = useQuery({ queryKey: queryKeys.sessions, queryFn: () => api.list_sessions(), enabled: !!session })
  const row = session ? sessions.data?.find((s) => s.id === session) : undefined
  // 79 §6.5: the owner engine is a sandbox VM (`SessionSummary.owner` ↔ `SandboxInfo.engine`) → its desktop is one click away.
  const sandboxes = useSandboxes()
  const vm = row?.owner ? sandboxes.data?.find((s) => s.engine === row.owner && s.vm?.desktop?.enabled) : undefined
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
  const [picker, setPicker] = useState(false)
  const [memory, setMemory] = useState(false)
  // 183 §4.5: the session's effective value on its owner (pin, else that machine's `tools.approvals`), the ⋯ menu's
  // reader (its setters invalidate the `['approvals', session]` prefix; a flip of this engine's switch re-reads);
  // this engine's own switch until it answers, or with the owner offline.
  const effective = useQuery({ queryKey: ['approvals', session, info?.approvals.global], queryFn: () => api.get_approvals(session ?? null), enabled: !!session, retry: false })
  const approvalsOff = pillOff(effective.data, info?.approvals ?? null)
  const mac = isMacDesktop()
  const ref = useRef<HTMLElement>(null)
  const rowRef = useRef<HTMLDivElement>(null)
  const tier = useTier(ref, rowRef)
  const compact = tier > 0

  const raw = session ? liveTitle || (row ? chatName(row) : 'Untitled chat') : 'New chat'
  const title = parseNote(raw)?.body || parseSpoken(raw)?.text || previewText(raw)
  // A popped window has no sidebar to open and is already its own window: no toggle, no search, no popout button.
  const popped = isPopout()
  const nav = first && (!open || isMobile) && !popped
  /** The talk view (B3): the header carries the way back to the same session's chat; the talk keeps running. */
  const chat = popout?.kind === 'talk'
  /**
   * The one panel button the compact bar keeps: artifacts (the session's output surface, with its count) over the
   * live browser over terminals (a job detail); the others move behind the chevron.
   */
  const panel = artifacts?.size ? 'artifacts' : codemap ? 'codemap' : browser ? 'browser' : terminals ? 'terminals' : null
  const { bar, menu } = layout(headerOrder(panel, canPopout() && !popped, chat, !!observed), tier)

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
    model: (mode) =>
      mode === 'row' ? (
        <ModelRow session={session} onOpen={() => setPicker(true)} />
      ) : (
        <div className="min-w-0">
          <ModelMenu session={session} />
        </div>
      ),
    // `w-0 flex-1`: takes the slack without counting towards the bar's natural width, so a long title never pushes
    // the controls out; it truncates to nothing before they move (or, in the full bar, before the tier drops).
    title: () => (
      <span data-tauri-drag-region={mac || undefined} className="flex w-0 min-w-0 flex-1 items-center gap-1.5 truncate text-sm font-medium" title={title}>
        {observed && <EyeMark className="size-3.5" />}
        <span className="truncate">{title}</span>
      </span>
    ),
    memory: (mode) => (session ? <MemoryBadge session={session} summary={row} row={mode === 'row'} open={memory} onToggle={() => setMemory((o) => !o)} /> : null),
    parent: (mode) => (parent ? <Parent id={parent} mode={mode} observed={observed} /> : null),
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
    desktop: (mode) =>
      !vm || !canPopout() ? null : mode === 'row' ? (
        <MenuRow icon={Monitor} label="Desktop" detail={vm.name} onSelect={() => void openPopout({ kind: 'vm', name: vm.name })} />
      ) : (
        <Button variant="ghost" size="sm" className="rounded-lg text-fg-secondary" aria-label={`Desktop of ${vm.name}`} onClick={() => void openPopout({ kind: 'vm', name: vm.name })}>
          <Monitor className="size-5" />
          {!compact && vm.name}
        </Button>
      ),
    context: (mode) =>
      !session ? null : mode === 'row' ? (
        <MenuRow icon={Gauge} label="Context" detail={<ContextMeter session={session} />} />
      ) : (
        <ContextMeter session={session} compact={mode === 'bar'} />
      ),
    cost: (mode) => (session ? <CostPill session={session} row={mode === 'row'} onOpen={() => setCost(true)} /> : null),
    terminals: (mode) => (session ? <TerminalButton session={session} row={mode === 'row'} onOpen={() => setTerminal(true)} /> : null),
    browser: (mode) =>
      !session || !browser ? null : mode === 'row' ? (
        <MenuRow icon={<BrowserIndicator live={browserBusy} className="text-fg-secondary" />} label="Live browser" onSelect={() => useSessions.getState().setBrowserLive(session, true)} />
      ) : (
        <Button
          variant="ghost"
          className="size-9 rounded-lg text-fg-secondary"
          aria-label="Live browser"
          onClick={() => useSessions.getState().setBrowserLive(session, true)}
        >
          <BrowserIndicator live={browserBusy} size={20} />
        </Button>
      ),
    artifacts: (mode) => (session ? <Artifacts session={session} row={mode === 'row'} /> : null),
    codemap: (mode) => (session && codemap ? <CodeMapButton session={session} row={mode === 'row'} /> : null),
    approvalsOff: (mode) =>
      !approvalsOff ? null : mode === 'row' ? (
        <MenuRow icon={ShieldOff} label="Approvals off" />
      ) : (
        <Badge variant="outline" className="text-fg-secondary">
          approvals off
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
      if (!session) return null
      const target = popout ?? { kind: 'chat', session }
      return mode === 'row' ? (
        <MenuRow icon={AppWindow} label="Open in new window" onSelect={() => void openPopout(target)} />
      ) : (
        <Button variant="ghost" className="size-9 rounded-lg text-fg-secondary" aria-label="Open in new window" onClick={() => void openPopout(target)}>
          <AppWindow className="size-5" />
        </Button>
      )
    },
    menu: () => (session ? <SessionMenu session={session} /> : null),
    close: () =>
      onClose ? (
        <Button variant="ghost" size="icon" className="rounded-lg text-fg-secondary" aria-label="Close pane" onClick={onClose}>
          <X className="size-5" />
        </Button>
      ) : null,
  }

  return (
    <header ref={ref} data-header-tier={tier} className="relative flex h-[52px] shrink-0 overflow-hidden">
      <div
        ref={rowRef}
        data-tauri-drag-region={mac || undefined}
        className={cn('flex h-full w-full min-w-max items-center', compact ? 'gap-1 px-2' : 'gap-2 px-3', nav && !open && mac && 'pl-20')}
      >
        {bar.map((i) => (
          <Fragment key={i.id}>{render[i.id](compact ? 'bar' : 'full')}</Fragment>
        ))}
      </div>
      <ApprovalsSheet open={sheet} onOpenChange={setSheet} />
      {compact && <ModelDialog session={session} open={picker} onOpenChange={setPicker} />}
      {session && (
        <>
          <CostDialog session={session} open={cost} onOpenChange={setCost} />
          <TerminalViewer session={session} open={terminal} onOpenChange={setTerminal} />
          <BrowserLive session={session} open={!!live} onOpenChange={(o) => useSessions.getState().setBrowserLive(session, o)} />
          <ArtifactPanel session={session} />
          <CodeMapSheet session={session} />
          <MemoryPanel session={session} open={memory} onOpenChange={setMemory} />
          {/* 188: here, not in the ⋯ menu, so it outlives the menu closing; `?messaging=1` opens it too. */}
          <MessagingSheet session={session} />
        </>
      )}
    </header>
  )
}

export function SidebarToggle() {
  const { toggleSidebar } = useSidebar()
  return (
    <Button
      variant="ghost"
      size="icon"
      className="rounded-lg text-fg-secondary"
      aria-label="Open sidebar"
      onClick={toggleSidebar}
    >
      <PanelLeft className="size-5" />
    </Button>
  )
}
