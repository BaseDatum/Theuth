import { AppWindow, ArrowLeft, ArrowRight, ExternalLink, Globe, RotateCw, X } from 'lucide-react'
import { type ReactNode, type RefObject, useEffect, useRef, useState } from 'react'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip'
import { useEngines } from '@/link/engines'
import { isLocal } from '@/lib/files'
import { usePanes } from '@/lib/panes'
import { openExternal } from '@/lib/platform'
import { canPopout, openPopout } from '@/lib/popout'
import { cn } from '@/lib/utils'
import * as native from '@/lib/web-view-native'
import { isLoopback, normalizeUrl, useWebViews, webLeaf } from '@/lib/web-views'

/** Open overlays a native web view would draw over (it sits above all DOM): radix dialogs, sheets, menus, popovers,
 * selects and the modal backdrops. Tooltips are not among them: they come and go with the pointer. */
const OVERLAYS = [
  '[role="dialog"]',
  '[role="alertdialog"]',
  '[role="menu"]',
  '[role="listbox"]',
  '[data-radix-popper-content-wrapper]',
  '[data-slot="sheet-content"]',
  '[data-slot$="-overlay"]',
].join(',')

const isTooltip = (el: Element) => el.querySelector('[role="tooltip"],[data-slot="tooltip-content"]') !== null

const intersects = (a: DOMRect, b: DOMRect) => a.left < b.right && b.left < a.right && a.top < b.bottom && b.top < a.bottom

/** Some open overlay intersects `ref`'s box. Watches `document.body` for overlays mounting, unmounting or changing state. */
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
    const schedule = () => {
      if (!frame) frame = requestAnimationFrame(check)
    }
    const mo = new MutationObserver(schedule)
    mo.observe(document.body, { childList: true, subtree: true, attributes: true, attributeFilter: ['data-state', 'style', 'hidden'] })
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

const warn = (what: string) => (e: unknown) => console.warn(what, e)

function Tip({ label, children }: { label: string; children: ReactNode }) {
  return (
    <Tooltip>
      <TooltipTrigger asChild>{children}</TooltipTrigger>
      <TooltipContent>{label}</TooltipContent>
    </Tooltip>
  )
}

/**
 * A web view frame (205 §4.4): a 52 px toolbar over a placeholder the shell's child web view `web-<id>` is laid over.
 * Mounting attaches it at the placeholder's rectangle, a resize or scroll moves it (once per frame), unmounting hides
 * it; the store's `close` destroys it. Where there is no native layer (a browser, iOS) or the address is on a remote
 * engine's loopback, the placeholder shows the address with **Open in browser** instead (205 D7). `popped`: the frame
 * fills its own window (`/web/$id`), whose title bar closes it.
 */
export function WebFrame({ id, onClose, popped = false }: { id: string; onClose?: () => void; popped?: boolean }) {
  const view = useWebViews((s) => s.views[id])
  const active = useEngines((s) => s.active)
  const engine = useEngines((s) => s.engines.find((e) => e.id === s.active)?.name ?? s.active)
  const [forced, setForced] = useState(false)
  const [draft, setDraft] = useState<string | null>(null)
  const ref = useRef<HTMLDivElement>(null)
  const hasNative = native.nativeWebViews()
  const current = view?.current ?? ''
  const remote = hasNative && !isLocal(active) && isLoopback(current)
  const live = view !== undefined && hasNative && (!remote || forced)
  const occluded = useOccluded(ref, live)

  useEffect(() => {
    const el = ref.current
    if (!live || !el) return
    const start = useWebViews.getState().views[id]
    void native.attach(id, start?.current || start?.url || '', rectOf(el)).catch(warn('web_view_attach'))
    let frame = 0
    const schedule = () => {
      if (!frame)
        frame = requestAnimationFrame(() => {
          frame = 0
          void native.bounds(id, rectOf(el)).catch(warn('web_view_bounds'))
        })
    }
    const ro = new ResizeObserver(schedule)
    ro.observe(el)
    window.addEventListener('resize', schedule)
    window.addEventListener('scroll', schedule, true)
    return () => {
      cancelAnimationFrame(frame)
      ro.disconnect()
      window.removeEventListener('resize', schedule)
      window.removeEventListener('scroll', schedule, true)
      void native.visible(id, false).catch(warn('web_view_visible'))
    }
  }, [id, live])

  // Hide under an overlay or a drag; show again (at the current rectangle) once it is gone. The attach above shows it.
  const wasOccluded = useRef(false)
  useEffect(() => {
    const el = ref.current
    if (!live || !el || occluded === wasOccluded.current) return
    wasOccluded.current = occluded
    if (occluded) void native.visible(id, false).catch(warn('web_view_visible'))
    else {
      void native.bounds(id, rectOf(el)).catch(warn('web_view_bounds'))
      void native.visible(id, true).catch(warn('web_view_visible'))
    }
  }, [id, live, occluded])

  useEffect(() => {
    if (!live) return
    const label = native.webLabel(id)
    let off: (() => void) | undefined
    let gone = false
    void native.onState((s) => {
      if (s.label === label) useWebViews.getState().patch(id, { current: s.url, title: s.title || useWebViews.getState().views[id]?.title, loading: s.loading })
    }).then((u) => (gone ? u() : (off = u)))
    return () => {
      gone = true
      off?.()
    }
  }, [id, live])

  if (!view) {
    return (
      <div className="flex min-h-0 flex-1 flex-col">
        <header className="relative flex h-[52px] shrink-0 items-center justify-end gap-2 px-3">
          {onClose && !popped && <CloseButton onClose={onClose} />}
        </header>
        <div className="flex flex-1 items-center justify-center text-sm text-fg-secondary">This web view is closed.</div>
      </div>
    )
  }

  const go = (input: string) => {
    const url = normalizeUrl(input)
    setDraft(null)
    useWebViews.getState().patch(id, { url, current: url })
    if (live) void native.navigate(id, url).catch(warn('web_view_navigate'))
  }
  const popOut = () => {
    useWebViews.getState().patch(id, { popped: true })
    void openPopout({ kind: 'web', id, url: current, title: view.title })
    usePanes.getState().close(webLeaf(id))
  }

  return (
    <div className="flex min-h-0 min-w-0 flex-1 flex-col">
      <header className="relative flex h-[52px] shrink-0 items-center gap-1 px-2">
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
          className="mx-1 h-8 min-w-0 flex-1 font-mono text-xs"
          spellCheck={false}
          value={draft ?? current}
          onChange={(e) => setDraft(e.target.value)}
          onBlur={() => setDraft(null)}
          onKeyDown={(e) => {
            if (e.key === 'Enter' && draft !== null && draft.trim()) go(draft)
            else if (e.key === 'Escape') {
              setDraft(null)
              e.currentTarget.blur()
            }
          }}
        />
        <Tip label="Open in browser">
          <Button variant="ghost" size="icon" className="rounded-lg text-fg-secondary" aria-label="Open in browser" onClick={() => openExternal(current)}>
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
              <Button variant="outline" size="sm" onClick={() => openExternal(current)}>
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
