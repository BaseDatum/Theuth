import { AppWindow, ArrowLeft, ArrowRight, Bot, ExternalLink, Globe, RotateCw, Square, X } from 'lucide-react'
import { type ReactNode, useEffect, useRef, useState } from 'react'
import { useNativeLayer, warn } from '@/components/chat/web-native-frame'
import { SidebarToggle } from '@/components/chat/SidebarToggle'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { useSidebar } from '@/components/ui/sidebar'
import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip'
import { useViewTitle } from '@danfessler/trellis-react'
import { useViewDrag } from '@/hooks/use-view-drag'
import { useEngines } from '@/link/engines'
import { isMacDesktop } from '@/lib/hotkeys'
import { container } from '@/lib/platform'
import { popOutWeb } from '@/lib/panes'
import { canPopout, closePopout, isPopout } from '@/lib/popout'
import { cn } from '@/lib/utils'
import { remoteLoopback } from '@/lib/web-arrival'
import { showFrame, stopAgent, useAgentActing, useTileAgent } from '@/lib/web-view-agent'
import * as native from '@/lib/web-view-native'
import { httpUrl, openInBrowser, placeTile, useWebViews } from '@/lib/web-views'

/** Toolbar tooltips open sideways, over the toolbar's DOM: below it the native view would cover them. */
function Tip({ label, children }: { label: string; children: ReactNode }) {
  return (
    <Tooltip>
      <TooltipTrigger asChild>{children}</TooltipTrigger>
      <TooltipContent side="left">{label}</TooltipContent>
    </Tooltip>
  )
}

/**
 * A web view frame (205 §4.4): a 52 px toolbar over a placeholder the shell's native web view `web-<id>` is laid over
 * (a child webview on the desktop, a `WKWebView` on iOS, 205 §9). Mounting attaches it at the placeholder's rectangle,
 * a per-frame check follows the placeholder while it shows, unmounting hides it; the store's `close` destroys it. Where
 * there is no native layer (a browser) or the address is on a remote engine's loopback, the placeholder shows the
 * address with **Open in browser** instead (205 D7).
 * `first`: the left-most pane, which carries the sidebar toggle and the traffic-light inset like `Header`. `popped`:
 * the frame fills its own window (`/web/$id`), whose title bar closes it.
 */
export function WebFrame({ id, first = false, onClose, popped = false }: { id: string; first?: boolean; onClose?: () => void; popped?: boolean }) {
  const { view, current, live, remote, engine, force } = useLiveFrame(id)
  const acting = useAgentActing(id)
  const stopping = useTileAgent((s) => s.tiles[id]?.stopping ?? false)
  const { open, isMobile, setOpen } = useSidebar()
  const ref = useRef<HTMLDivElement>(null)
  const occluded = useNativeLayer(id, ref, live, (e) => {
    // 205 D6: a pop-out window holds the page; focus it rather than leave an empty tile here.
    if (!popped && String(e).startsWith('in-popout:')) {
      const v = useWebViews.getState().views[id]
      if (v) void popOutWeb({ id, session: v.session, url: v.current || v.url, title: v.title }).catch(warn('open_window'))
    } else warn('web_view_attach')(e)
  })
  const mac = isMacDesktop()
  // Beside other panes the toolbar drags this one (`useViewDrag`); the address field and buttons stay interactive.
  const drag = useViewDrag()
  // The tab (shown while web views share a panel): the page's title, else its host.
  useViewTitle(view?.title || hostOf(current) || 'Web')
  const region = (mac && drag.region) || undefined
  // `Header`'s rule: a popped window has no sidebar to open and is its own window.
  const nav = first && (!open || isMobile) && !popped && !isPopout()

  // 209 D11: a tile placed too narrow for its address field on the iPad folds the inline sidebar away, once per frame,
  // decided at its first real width (a pane not laid out yet measures 0).
  const shown = view !== undefined
  useEffect(() => {
    const el = ref.current
    if (!shown || popped || !el) return
    const decide = () => {
      if (placeTile(id, { width: el.getBoundingClientRect().width, sidebarOpen: open, phone: isMobile, ios: container() === 'ios' })) setOpen(false)
    }
    decide()
    if (typeof ResizeObserver !== 'function') return
    const observer = new ResizeObserver(decide)
    observer.observe(el)
    return () => observer.disconnect()
  }, [id, shown, popped, open, isMobile, setOpen])

  // 209 §9: while the page shows here the bar below tells of the agent; otherwise the window-level banner does.
  useEffect(() => (live && !occluded ? showFrame(id) : undefined), [id, live, occluded])

  // The main window keeps every frame's state (`installWebViewEvents`); a pop-out window listens for its own frame,
  // and closes once its view is destroyed (the main window closed the frame: its session deleted, the engine left).
  useEffect(() => {
    if (!live || !popped) return
    const label = native.webLabel(id)
    const offs: (() => void)[] = []
    let gone = false
    const keep = (p: Promise<() => void>) => void p.then((u) => (gone ? u() : offs.push(u)))
    keep(
      native.onState((s) => {
        const v = useWebViews.getState().views[id]
        if (s.label === label && v) useWebViews.getState().patch(id, { current: s.url, title: s.title || v.title, loading: s.loading })
      }),
    )
    keep(native.onClosed((s) => s.label === label && closePopout()))
    return () => {
      gone = true
      for (const off of offs) off()
    }
  }, [id, live, popped])

  const toggle = nav ? <SidebarToggle /> : null
  const rowClass = cn('relative flex h-[52px] shrink-0 items-center gap-1 px-2', nav && !open && mac && 'pl-20')

  if (!view) {
    return (
      <div className="flex min-h-0 flex-1 flex-col">
        <header {...drag.props} data-tauri-drag-region={region} className={cn(rowClass, 'justify-between')}>
          {toggle}
          {onClose && !popped && <CloseButton onClose={onClose} />}
        </header>
        <div className="flex flex-1 items-center justify-center text-sm text-fg-secondary">This web view is closed.</div>
      </div>
    )
  }

  const popOut = () => void popOutWeb({ id, session: view.session, url: current, title: view.title }).catch(warn('open_window'))

  return (
    <div className="flex min-h-0 min-w-0 flex-1 flex-col">
      <header {...drag.props} data-tauri-drag-region={region} className={rowClass}>
        {toggle}
        <Tip label="Back">
          <Button variant="ghost" size="icon" className="rounded-lg text-fg-secondary" aria-label="Back" disabled={!live} onClick={() => void native.history(id, -1).catch(warn('web_view_history'))}>
            <ArrowLeft className="size-5" />
          </Button>
        </Tip>
        <Tip label="Forward">
          <Button variant="ghost" size="icon" className="rounded-lg text-fg-secondary" aria-label="Forward" disabled={!live} onClick={() => void native.history(id, 1).catch(warn('web_view_history'))}>
            <ArrowRight className="size-5" />
          </Button>
        </Tip>
        <Tip label="Reload">
          <Button variant="ghost" size="icon" className="rounded-lg text-fg-secondary" aria-label="Reload" disabled={!live} onClick={() => void native.reload(id).catch(warn('web_view_reload'))}>
            <RotateCw className={cn('size-5', view.loading && 'animate-spin')} />
          </Button>
        </Tip>
        <AddressField id={id} live={live} current={current} className="mx-1 h-8 text-xs" />
        <Tip label="Open in browser">
          <Button variant="ghost" size="icon" className="rounded-lg text-fg-secondary" aria-label="Open in browser" onClick={() => openInBrowser(current)}>
            <ExternalLink className="size-5" />
          </Button>
        </Tip>
        {!popped && canPopout() && (
          <Button variant="ghost" className="size-9 rounded-lg text-fg-secondary" aria-label="Open in new window" onClick={popOut}>
            <AppWindow className="size-5" />
          </Button>
        )}
        {onClose && !popped && <CloseButton onClose={onClose} />}
      </header>
      {acting && <AgentBar stopping={stopping} onStop={() => stopAgent(id)} />}
      <div ref={ref} data-web-frame={id} className="relative min-h-0 flex-1 border-t bg-muted/40">
        <FrameFace live={live} occluded={occluded} remote={remote} engine={engine} title={view.title} url={current} onForce={force} />
      </div>
    </div>
  )
}

/** `url`'s host, or `''` for no valid address. */
function hostOf(url: string): string {
  try {
    return new URL(url).host
  } catch {
    return ''
  }
}

/** Frame `id` as this window shows it: its store entry, its address, and whether its native layer is live here —
 * there is one (205 D7) and the address is not on a remote engine's loopback, unless the user chose **Load anyway**
 * (`force`, 205 §4.4). */
export function useLiveFrame(id: string) {
  const view = useWebViews((s) => s.views[id])
  const active = useEngines((s) => s.active)
  const engine = useEngines((s) => s.engines.find((e) => e.id === s.active)?.name ?? s.active)
  const [forced, setForced] = useState(false)
  const hasNative = native.nativeWebViews()
  const current = view?.current ?? ''
  const remote = hasNative && remoteLoopback(active, current)
  const live = view !== undefined && hasNative && (!remote || forced)
  return { view, current, live, remote, engine, force: () => setForced(true) }
}

/** The address field (205 §4.4): Enter navigates an http(s) address (`httpUrl`); anything else marks the field and
 * stays. `current` follows the page: the shell's state event, or the navigation having been accepted. `compact` (the
 * phone, 209 §7): no dictation mic in the narrow field, a URL keyboard, and the field lets go once it navigated, so the
 * keyboard goes down over the page. */
export function AddressField({ id, live, current, className, compact = false }: { id: string; live: boolean; current: string; className?: string; compact?: boolean }) {
  const [draft, setDraft] = useState<string | null>(null)
  const [invalid, setInvalid] = useState(false)
  const go = (input: string, field: HTMLInputElement) => {
    const url = httpUrl(input)
    if (!url) {
      setInvalid(true)
      return
    }
    setDraft(null)
    setInvalid(false)
    if (compact) field.blur()
    if (live) native.navigate(id, url).then(() => useWebViews.getState().patch(id, { url, current: url }), warn('web_view_navigate'))
    else useWebViews.getState().patch(id, { url, current: url })
  }
  return (
    <Input
      aria-label="Address"
      aria-invalid={invalid || undefined}
      title={invalid ? 'Only http and https addresses open here' : undefined}
      className={cn('min-w-0 flex-1 font-mono', className)}
      spellCheck={false}
      {...(compact && { dictate: false, inputMode: 'url' as const, enterKeyHint: 'go' as const, autoCapitalize: 'off', autoCorrect: 'off' })}
      value={draft ?? current}
      // A long address in a narrow field: a tap selects it all, so typing replaces it (as Safari's address bar).
      onFocus={(e) => {
        const el = e.currentTarget
        if (compact) requestAnimationFrame(() => el.setSelectionRange(0, el.value.length))
      }}
      onChange={(e) => {
        setDraft(e.target.value)
        setInvalid(false)
      }}
      onBlur={() => {
        setDraft(null)
        setInvalid(false)
      }}
      onKeyDown={(e) => {
        if (e.key === 'Enter' && draft !== null && draft.trim()) go(draft, e.currentTarget)
        else if (e.key === 'Escape') {
          setDraft(null)
          setInvalid(false)
          e.currentTarget.blur()
        }
      }}
    />
  )
}

/** The placeholder's content: nothing while the native layer shows; its title and address while an overlay hides it;
 * where there is no layer, the address with **Open in browser** (and **Load anyway** for a remote engine's loopback). */
export function FrameFace(p: { live: boolean; occluded: boolean; remote: boolean; engine: string; title: string; url: string; onForce: () => void }) {
  if (p.live) return p.occluded ? <Quiet title={p.title} url={p.url} /> : null
  return (
    <Quiet title={p.title} url={p.url}>
      {p.remote && <p className="text-sm text-fg-secondary">This address is on {p.engine}.</p>}
      <div className="flex gap-2">
        <Button variant="outline" size="sm" onClick={() => openInBrowser(p.url)}>
          <ExternalLink />
          Open in browser
        </Button>
        {p.remote && (
          <Button variant="ghost" size="sm" onClick={p.onForce}>
            Load anyway
          </Button>
        )}
      </div>
    </Quiet>
  )
}

/** 209 §9: while an agent call on this tile is in flight (and `LINGER_MS` after), a slim bar under the toolbar; Stop
 * fails the call, stops the page ("Stopping…" until the shell's `stop` settles) and refuses the tile's next calls for a
 * minute. Between the toolbar and the placeholder, so the native view (laid over the placeholder) moves down for it
 * rather than covering it. */
export function AgentBar({ stopping, onStop }: { stopping: boolean; onStop: () => void }) {
  return (
    <div role="status" aria-label="Agent is using this page" className="flex h-9 shrink-0 items-center gap-2 border-t bg-accent px-3 text-xs">
      <Bot className="size-4 shrink-0 text-fg-secondary" aria-hidden="true" />
      <span className="min-w-0 flex-1 truncate">{stopping ? 'Stopping…' : 'Agent is using this page'}</span>
      <Button variant="outline" size="xs" className="pointer-coarse:min-h-9" aria-label="Stop the agent on this page" disabled={stopping} onClick={onStop}>
        <Square aria-hidden="true" />
        Stop
      </Button>
    </div>
  )
}

function CloseButton({ onClose }: { onClose: () => void }) {
  return (
    <Button variant="ghost" size="icon" className="rounded-lg text-fg-secondary" aria-label="Close pane" onClick={onClose}>
      <X className="size-5" />
    </Button>
  )
}

/** The placeholder's own face: while the native layer is hidden, or where there is none. */
export function Quiet({ title, url, children }: { title: string; url: string; children?: ReactNode }) {
  return (
    <div className="absolute inset-0 flex flex-col items-center justify-center gap-3 p-6 text-center">
      <Globe className="size-8 text-fg-secondary" />
      <div className="max-w-full truncate font-medium">{title}</div>
      <div className="max-w-full truncate font-mono text-xs text-fg-secondary">{url}</div>
      {children}
    </div>
  )
}
