import { useQuery } from '@tanstack/react-query'
import { Link, useNavigate } from '@tanstack/react-router'
import { AppWindow, CornerLeftUp, Gauge, LayoutGrid, Network, PanelLeft, Search, ShieldOff, X } from 'lucide-react'
import { Fragment, type ReactNode, type RefObject, useLayoutEffect, useRef, useState } from 'react'
import { flushSync } from 'react-dom'
import { SearchTrigger } from '@/components/sidebar/SearchTrigger'
import { EngineChip } from '@/components/sidebar/SessionItem'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuTrigger } from '@/components/ui/dropdown-menu'
import { useSidebar } from '@/components/ui/sidebar'
import { chatName } from '@/lib/attach'
import { type HeaderId, type Mode, decideTier, headerOrder, layout } from '@/lib/header'
import { isMacDesktop } from '@/lib/hotkeys'
import { canPopout, isPopout, openPopout } from '@/lib/popout'
import { useEngineLabel } from '@/lib/owner'
import { queryKeys, useApi } from '@/lib/rpc'
import { cn } from '@/lib/utils'
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
import { BrowserIndicator } from './Indicators'
import { SessionMenu } from './SessionMenu'
import { TerminalButton, TerminalViewer, useHasTerminals } from './TerminalViewer'

/** `↳ parent` breadcrumb of a subagent's transcript; `row`: the compact menu's "Parent" line. */
function Parent({ id, row }: { id: string; row?: boolean }) {
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
        icon={CornerLeftUp}
        label="Parent"
        detail={name}
        onSelect={() => void navigate({ to: '/c/$session', params: { session: id } })}
      />
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

/** `name · epoch N` for the compact menu's Owner line. */
function OwnerDetail({ owner, epoch }: { owner: string; epoch: number }) {
  const label = useEngineLabel(owner)
  return <span title={`owned by ${owner}`}>{`${label} · epoch ${epoch}`}</span>
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
  draftEffort,
  onDraftEffort,
  first = true,
  onClose,
}: {
  session: string | null
  draftEffort?: string | null
  onDraftEffort?(effort: string | null): void
  first?: boolean
  onClose?(): void
}) {
  const api = useApi()
  const { open, isMobile } = useSidebar()
  const info = useConnection((s) => s.info)
  const pending = useApprovals((s) => s.pending.length)
  const parent = useSessionField(session ?? '', 'parent')
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
  const [sheet, setSheet] = useState(false)
  const [cost, setCost] = useState(false)
  const [terminal, setTerminal] = useState(false)
  const [picker, setPicker] = useState(false)
  const approvalsOff = info !== null && (!info.approvals.global || !info.approvals.session)
  const mac = isMacDesktop()
  const ref = useRef<HTMLElement>(null)
  const rowRef = useRef<HTMLDivElement>(null)
  const tier = useTier(ref, rowRef)
  const compact = tier > 0

  const raw = session ? liveTitle || (row ? chatName(row) : 'Untitled chat') : 'New chat'
  const title = parseNote(raw)?.body || raw
  // A popped window has no sidebar to open and is already its own window: no toggle, no search, no popout button.
  const popped = isPopout()
  const nav = first && (!open || isMobile) && !popped
  /**
   * The one panel button the compact bar keeps: artifacts (the session's output surface, with its count) over the
   * live browser over terminals (a job detail); the others move behind the chevron.
   */
  const panel = artifacts?.size ? 'artifacts' : codemap ? 'codemap' : browser ? 'browser' : terminals ? 'terminals' : null
  const { bar, menu } = layout(headerOrder(panel, canPopout() && !popped), tier)

  const render: Record<HeaderId, (mode: Mode) => ReactNode> = {
    search: (mode) =>
      !nav ? null : mode === 'row' ? (
        <MenuRow icon={Search} label="Search" onSelect={() => window.dispatchEvent(new Event('theuth:search'))} />
      ) : (
        <SearchTrigger className="shrink-0" />
      ),
    sidebar: () => (nav ? <SidebarToggle /> : null),
    model: (mode) =>
      mode === 'row' ? (
        <ModelRow session={session} draftEffort={draftEffort} onDraftEffort={onDraftEffort} onOpen={() => setPicker(true)} />
      ) : (
        <div className="min-w-0">
          <ModelMenu session={session} draftEffort={draftEffort} onDraftEffort={onDraftEffort} />
        </div>
      ),
    // `w-0 flex-1`: takes the slack without counting towards the bar's natural width, so a long title never pushes
    // the controls out; it truncates to nothing before they move (or, in the full bar, before the tier drops).
    title: () => (
      <span data-tauri-drag-region={mac || undefined} className="w-0 min-w-0 flex-1 truncate text-sm font-medium" title={title}>
        {title}
      </span>
    ),
    parent: (mode) => (parent ? <Parent id={parent} row={mode === 'row'} /> : null),
    owner: (mode) =>
      row && !row.here ? (
        mode === 'row' ? (
          <MenuRow icon={Network} label="Owner" detail={<OwnerDetail owner={row.owner} epoch={row.epoch} />} />
        ) : (
          <EngineChip owner={row.owner} epoch={row.epoch} />
        )
      ) : null,
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
    popout: (mode) =>
      !session ? null : mode === 'row' ? (
        <MenuRow icon={AppWindow} label="Open in new window" onSelect={() => void openPopout({ kind: 'chat', session })} />
      ) : (
        <Button
          variant="ghost"
          className="size-9 rounded-lg text-fg-secondary"
          aria-label="Open in new window"
          onClick={() => void openPopout({ kind: 'chat', session })}
        >
          <AppWindow className="size-5" />
        </Button>
      ),
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
      {compact && <ModelDialog session={session} draftEffort={draftEffort} onDraftEffort={onDraftEffort} open={picker} onOpenChange={setPicker} />}
      {session && (
        <>
          <CostDialog session={session} open={cost} onOpenChange={setCost} />
          <TerminalViewer session={session} open={terminal} onOpenChange={setTerminal} />
          <BrowserLive session={session} open={!!live} onOpenChange={(o) => useSessions.getState().setBrowserLive(session, o)} />
          <ArtifactPanel session={session} />
          <CodeMapSheet session={session} />
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
