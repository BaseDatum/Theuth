import { AppWindow, ArrowLeft, ArrowRight, ExternalLink, Globe, RotateCw, X } from 'lucide-react'
import { type ReactNode, type RefObject, useEffect, useRef, useState } from 'react'
import { SidebarToggle } from '@/components/chat/Header'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { useSidebar } from '@/components/ui/sidebar'
import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip'
import { useEngines } from '@/link/engines'
import { isLocal } from '@/lib/files'
import { isMacDesktop } from '@/lib/hotkeys'
import { popOutWeb, usePanes } from '@/lib/panes'
import { canPopout, closePopout, isPopout } from '@/lib/popout'
import { cn } from '@/lib/utils'
import * as native from '@/lib/web-view-native'
import { httpUrl, isLoopback, openInBrowser, useWebViews } from '@/lib/web-views'

/** Open things a native web view would draw over (it sits above all DOM): radix dialogs, sheets, menus, popovers,
 * selects, the modal backdrops and toasts. Tooltips are not among them: they come and go with the pointer. */
const OVERLAYS = [
  '[role="dialog"]',
  '[role="alertdialog"]',
  '[role="menu"]',
  '[role="listbox"]',
  '[data-radix-popper-content-wrapper]',
  '[data-slot="sheet-content"]',
  '[data-slot$="-overlay"]',
  '[data-sonner-toast]',
].join(',')

const isTooltip = (el: Element) => el.querySelector('[role="tooltip"],[data-slot="tooltip-content"]') !== null

const intersects = (a: DOMRect, b: DOMRect) => a.left < b.right && b.left < a.right && a.top < b.bottom && b.top < a.bottom

/** A mutation inside the transcript's virtual rows (streaming text, cards) never opens an overlay. */
const inTranscript = (n: Node) => (n instanceof Element ? n : n.parentElement)?.closest('[data-index]') != null

/** Some open overlay intersects `ref`'s box. Watches `document.body` for overlays mounting, unmounting or changing
 * state (`data-state`), one check per animation frame. */
function useOverlayOver(ref: RefObject<HTMLElement | null>, enabled: boolean): boolean {
  const [over, setOver] = useState(false)
  useEffect(() => {
    if (!enabled) {
      setOver(false)
      return
    }
    let frame = 0
    const check = () => {
      frame = 0
      const el = ref.current
      if (!el) return
      const box = el.getBoundingClientRect()
      setOver([...document.querySelectorAll(OVERLAYS)].some((o) => !el.contains(o) && !isTooltip(o) && intersects(o.getBoundingClientRect(), box)))
    }
    const mo = new MutationObserver((records) => {
      if (!frame && records.some((r) => !inTranscript(r.target))) frame = requestAnimationFrame(check)
    })
    mo.observe(document.body, { childList: true, subtree: true, attributes: true, attributeFilter: ['data-state'] })
    check()
    return () => {
      mo.disconnect()
      cancelAnimationFrame(frame)
    }
  }, [ref, enabled])
  return over
}

/** The native layer must not show: a pane drag is in flight (its drop zones are DOM) or an overlay covers the frame. */
function useOccluded(ref: RefObject<HTMLElement | null>, enabled: boolean): boolean {
  const dragging = usePanes((s) => s.drag !== null)
  return useOverlayOver(ref, enabled) || (enabled && dragging)
}

const rectOf = (el: HTMLElement): native.Rect => {
  const r = el.getBoundingClientRect()
  return { x: r.left, y: r.top, width: r.width, height: r.height }
}
const sameRect = (a: native.Rect, b: native.Rect | null) => b !== null && a.x === b.x && a.y === b.y && a.width === b.width && a.height === b.height

const warn = (what: string) => (e: unknown) => console.warn(what, e)

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
 * A web view frame (205 §4.4): a 52 px toolbar over a placeholder the shell's child web view `web-<id>` is laid over.
 * Mounting attaches it at the placeholder's rectangle, a per-frame check follows the placeholder while it shows,
 * unmounting hides it; the store's `close` destroys it. Where there is no native layer (a browser, iOS) or the address
 * is on a remote engine's loopback, the placeholder shows the address with **Open in browser** instead (205 D7).
 * `first`: the left-most pane, which carries the sidebar toggle and the traffic-light inset like `Header`. `popped`:
 * the frame fills its own window (`/web/$id`), whose title bar closes it.
 */
export function WebFrame({ id, first = false, onClose, popped = false }: { id: string; first?: boolean; onClose?: () => void; popped?: boolean }) {
  const view = useWebViews((s) => s.views[id])
  const active = useEngines((s) => s.active)
  const engine = useEngines((s) => s.engines.find((e) => e.id === s.active)?.name ?? s.active)
  const { open, isMobile } = useSidebar()
  const [forced, setForced] = useState(false)
  const [draft, setDraft] = useState<string | null>(null)
  const [invalid, setInvalid] = useState(false)
  const ref = useRef<HTMLDivElement>(null)
  const hasNative = native.nativeWebViews()
  const current = view?.current ?? ''
  const remote = hasNative && !isLocal(active) && isLoopback(current)
  const live = view !== undefined && hasNative && (!remote || forced)
  const occluded = useOccluded(ref, live)
  const mac = isMacDesktop()
  // `Header`'s rule: a popped window has no sidebar to open and is its own window.
  const nav = first && (!open || isMobile) && !popped && !isPopout()

  useEffect(() => {
    const el = ref.current
    if (!live || !el) return
    const start = useWebViews.getState().views[id]
    native.attach(id, start?.current || start?.url || '', rectOf(el)).catch((e: unknown) => {
      // 205 D6: a pop-out window holds the page; focus it rather than leave an empty tile here.
      if (!popped && String(e).startsWith('in-popout:')) {
        const v = useWebViews.getState().views[id]
        if (v) void popOutWeb({ id, session: v.session, url: v.current || v.url, title: v.title }).catch(warn('open_window'))
      } else warn('web_view_attach')(e)
    })
    return () => void native.detach(id).catch(warn('web_view_visible'))
  }, [id, live, popped])

  // Hide under an overlay or a drag, show again once it is gone; the module sends only what changed.
  useEffect(() => {
    if (live) void native.visible(id, !occluded).catch(warn('web_view_visible'))
  }, [id, live, occluded])

  // Follow the placeholder (resizes, splits, scrolls, sidebar moves) once per frame while it shows; send only changes.
  useEffect(() => {
    const el = ref.current
    if (!live || occluded || !el) return
    let last: native.Rect | null = rectOf(el)
    let frame = requestAnimationFrame(function tick() {
      const r = rectOf(el)
      if (!sameRect(r, last)) {
        last = r
        void native.bounds(id, r).catch(warn('web_view_bounds'))
      }
      frame = requestAnimationFrame(tick)
    })
    return () => cancelAnimationFrame(frame)
  }, [id, live, occluded])

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
        <header data-tauri-drag-region={mac || undefined} className={cn(rowClass, 'justify-between')}>
          {toggle}
          {onClose && !popped && <CloseButton onClose={onClose} />}
        </header>
        <div className="flex flex-1 items-center justify-center text-sm text-fg-secondary">This web view is closed.</div>
      </div>
    )
  }

  const go = (input: string) => {
    const url = httpUrl(input)
    if (!url) {
      setInvalid(true)
      return
    }
    setDraft(null)
    setInvalid(false)
    // `current` follows the page: the shell's state event, or the navigation having been accepted.
    if (live) native.navigate(id, url).then(() => useWebViews.getState().patch(id, { url, current: url }), warn('web_view_navigate'))
    else useWebViews.getState().patch(id, { url, current: url })
  }
  const popOut = () => void popOutWeb({ id, session: view.session, url: current, title: view.title }).catch(warn('open_window'))

  return (
    <div className="flex min-h-0 min-w-0 flex-1 flex-col">
      <header data-tauri-drag-region={mac || undefined} className={rowClass}>
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
        <Input
          aria-label="Address"
          aria-invalid={invalid || undefined}
          title={invalid ? 'Only http and https addresses open here' : undefined}
          className="mx-1 h-8 min-w-0 flex-1 font-mono text-xs"
          spellCheck={false}
          value={draft ?? current}
          onChange={(e) => {
            setDraft(e.target.value)
            setInvalid(false)
          }}
          onBlur={() => {
            setDraft(null)
            setInvalid(false)
          }}
          onKeyDown={(e) => {
            if (e.key === 'Enter' && draft !== null && draft.trim()) go(draft)
            else if (e.key === 'Escape') {
              setDraft(null)
              setInvalid(false)
              e.currentTarget.blur()
            }
          }}
        />
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
      <div ref={ref} data-web-frame={id} className="relative min-h-0 flex-1 border-t bg-muted/40">
        {live ? (
          occluded && <Quiet title={view.title} url={current} />
        ) : (
          <Quiet title={view.title} url={current}>
            {remote && <p className="text-sm text-fg-secondary">This address is on {engine}.</p>}
            <div className="flex gap-2">
              <Button variant="outline" size="sm" onClick={() => openInBrowser(current)}>
                <ExternalLink />
                Open in browser
              </Button>
              {remote && (
                <Button variant="ghost" size="sm" onClick={() => setForced(true)}>
                  Load anyway
                </Button>
              )}
            </div>
          </Quiet>
        )}
      </div>
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
function Quiet({ title, url, children }: { title: string; url: string; children?: ReactNode }) {
  return (
    <div className="absolute inset-0 flex flex-col items-center justify-center gap-3 p-6 text-center">
      <Globe className="size-8 text-fg-secondary" />
      <div className="max-w-full truncate font-medium">{title}</div>
      <div className="max-w-full truncate font-mono text-xs text-fg-secondary">{url}</div>
      {children}
    </div>
  )
}
