import { ArrowLeft, ArrowRight, ChevronDown, ExternalLink, RotateCw, X } from 'lucide-react'
import { type PointerEvent, useEffect, useRef } from 'react'
import { AddressField, AgentBar, FrameFace, useLiveFrame } from '@/components/chat/WebFrame'
import { rectOf, useNativeLayer, warn } from '@/components/chat/web-native-frame'
import { Button } from '@/components/ui/button'
import { closeWeb } from '@/lib/panes'
import { cn } from '@/lib/utils'
import { showFrame, stopAgent, useAgentActing, useTileAgent } from '@/lib/web-view-agent'
import * as native from '@/lib/web-view-native'
import { openInBrowser, useWebViews } from '@/lib/web-views'

/** 209 §7: a downward swipe on the toolbar of at least this many px minimises the page. */
export const SWIPE_MINIMISE_PX = 60

/** A toolbar swipe of `dx`, `dy` px (down positive) minimises: at least `SWIPE_MINIMISE_PX` down, and more down than
 * sideways (a sideways drag over the address field is no minimise). */
export function swipeMinimises(dx: number, dy: number): boolean {
  return dy >= SWIPE_MINIMISE_PX && dy > Math.abs(dx)
}

/**
 * The phone's web page (209 D11, §7): the frame `useWebViews.front` full screen over the chat, a 52 px toolbar (back,
 * forward, reload, address, open in Safari, minimise, close) under the safe area, the "Agent is using this page" bar
 * while an agent call runs, and the placeholder the native view is laid over (`useNativeLayer`, shared with
 * `WebFrame`). Minimise (the button, or a swipe down on the toolbar) hides the view into the pill (`WebPill`) without
 * destroying it; close destroys it. Mounted by `Panes` on a phone only.
 */
export function PhoneWebView() {
  const front = useWebViews((s) => (s.front !== null && s.views[s.front] ? s.front : null))
  // Keyed: another frame brought to the front attaches its own view; the one it replaced was hidden on unmount.
  return front === null ? null : <Page key={front} id={front} />
}

function Page({ id }: { id: string }) {
  const { view, current, live, remote, engine, force } = useLiveFrame(id)
  const acting = useAgentActing(id)
  const stopping = useTileAgent((s) => s.tiles[id]?.stopping ?? false)
  const ref = useRef<HTMLDivElement>(null)
  const occluded = useNativeLayer(id, ref, live, warn('web_view_attach'))
  // 209 §9: while the page shows here the bar below tells of the agent; otherwise the window-level banner does.
  useEffect(() => (live && !occluded ? showFrame(id) : undefined), [id, live, occluded])
  // The iOS shell frames the view through its scroll view's content offset, which WebKit's scroll-to-reveal and the
  // keyboard move while this fixed placeholder's rectangle stays put: the frame is sent again on every visual viewport
  // resize or scroll and every window scroll while the page shows (once per animation frame).
  useEffect(() => {
    const el = ref.current
    if (!live || occluded || !el) return
    let frame = 0
    const again = () => {
      if (frame) return
      frame = requestAnimationFrame(() => {
        frame = 0
        void native.reframe(id, rectOf(el)).catch(warn('web_view_bounds'))
      })
    }
    const viewport = window.visualViewport
    viewport?.addEventListener('resize', again)
    viewport?.addEventListener('scroll', again)
    window.addEventListener('scroll', again)
    return () => {
      cancelAnimationFrame(frame)
      viewport?.removeEventListener('resize', again)
      viewport?.removeEventListener('scroll', again)
      window.removeEventListener('scroll', again)
    }
  }, [id, live, occluded])
  /** The swipe in progress; `held`: the toolbar captured its pointer. */
  const swipe = useRef<{ pointer: number; x: number; y: number; held: boolean } | null>(null)
  /** A swipe just minimised: the click its release would give a toolbar button is swallowed. */
  const swiped = useRef(false)
  if (!view) return null

  const minimise = () => useWebViews.getState().minimise(id)
  /** The pointer lifted (`done`) or was cancelled: let the capture go; a finished swipe down far enough minimises. */
  const end = (e: PointerEvent<HTMLElement>, done = true) => {
    const s = swipe.current
    swipe.current = null
    if (s?.held && e.currentTarget.hasPointerCapture(e.pointerId)) e.currentTarget.releasePointerCapture(e.pointerId)
    if (!done || !s || s.pointer !== e.pointerId || !swipeMinimises(e.clientX - s.x, e.clientY - s.y)) return
    swiped.current = true
    setTimeout(() => (swiped.current = false), 400)
    minimise()
  }
  // A finger or a pointer that starts to move takes the toolbar's pointer capture: the 60 px swipe leaves the 52 px
  // toolbar, and without capture a mouse or trackpad would release over the page, where the toolbar never hears it.
  // Not on press: a captured press would give its click to the toolbar instead of the button under it.
  const move = (e: PointerEvent<HTMLElement>) => {
    const s = swipe.current
    if (!s || s.held || s.pointer !== e.pointerId || Math.max(Math.abs(e.clientX - s.x), Math.abs(e.clientY - s.y)) < 4) return
    s.held = true
    e.currentTarget.setPointerCapture(e.pointerId)
  }
  const icon = 'size-9 shrink-0 rounded-lg text-fg-secondary'

  return (
    <div
      role="region"
      aria-label="Web page"
      data-phone-web={id}
      className="fixed inset-x-0 top-0 z-40 flex h-band flex-col bg-background pt-[var(--safe-top)] pb-[var(--safe-bottom)]"
    >
      <header
        className="flex h-[52px] shrink-0 touch-none items-center gap-0.5 pr-[max(0.25rem,var(--safe-right))] pl-[max(0.25rem,var(--safe-left))]"
        onPointerDown={(e) => (swipe.current = { pointer: e.pointerId, x: e.clientX, y: e.clientY, held: false })}
        onPointerMove={move}
        onPointerUp={(e) => end(e)}
        onPointerCancel={(e) => end(e, false)}
        onClickCapture={(e) => {
          if (!swiped.current) return
          e.preventDefault()
          e.stopPropagation()
        }}
      >
        <Button variant="ghost" size="icon" className={icon} aria-label="Back" disabled={!live} onClick={() => void native.history(id, -1).catch(warn('web_view_history'))}>
          <ArrowLeft className="size-5" />
        </Button>
        <Button variant="ghost" size="icon" className={icon} aria-label="Forward" disabled={!live} onClick={() => void native.history(id, 1).catch(warn('web_view_history'))}>
          <ArrowRight className="size-5" />
        </Button>
        <Button variant="ghost" size="icon" className={icon} aria-label="Reload" disabled={!live} onClick={() => void native.reload(id).catch(warn('web_view_reload'))}>
          <RotateCw className={cn('size-5', view.loading && 'animate-spin')} />
        </Button>
        {/* 16 px: iOS zooms the page into a smaller focused field. */}
        <AddressField id={id} live={live} current={current} compact className="mx-0.5 h-9 px-2 text-[16px] md:text-[16px]" />
        <Button variant="ghost" size="icon" className={icon} aria-label="Open in Safari" onClick={() => openInBrowser(current)}>
          <ExternalLink className="size-5" />
        </Button>
        <Button variant="ghost" size="icon" className={icon} aria-label="Minimise" onClick={minimise}>
          <ChevronDown className="size-5" />
        </Button>
        <Button variant="ghost" size="icon" className={icon} aria-label="Close" onClick={() => closeWeb(id)}>
          <X className="size-5" />
        </Button>
      </header>
      {acting && <AgentBar stopping={stopping} onStop={() => stopAgent(id)} />}
      <div ref={ref} data-web-frame={id} className="relative min-h-0 flex-1 border-t bg-muted/40">
        <FrameFace live={live} occluded={occluded} remote={remote} engine={engine} title={view.title} url={current} onForce={force} />
      </div>
    </div>
  )
}
