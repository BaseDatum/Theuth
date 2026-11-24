// 205 in the inbox: a `show_web` page as the detail pane — the live page itself (the frame's native web view laid over
// the pane, `useNativeLayer`), its title and address above, and the footer of what to do with it.
import { useNavigate } from '@tanstack/react-router'
import { AppWindow, ArrowLeft, ArrowRight, Check, Copy, ExternalLink, MessageSquare, RotateCw } from 'lucide-react'
import { useEffect, useRef, useState } from 'react'
import { openWebView } from '@/components/chat/WebViewCard'
import { AddressField, FrameFace, Quiet, useLiveFrame } from '@/components/chat/WebFrame'
import { useNativeLayer, warn } from '@/components/chat/web-native-frame'
import { Button } from '@/components/ui/button'
import { useIsMobile } from '@/hooks/use-mobile'
import { popOutWeb } from '@/lib/panes'
import { canPopout } from '@/lib/popout'
import { cn } from '@/lib/utils'
import * as native from '@/lib/web-view-native'
import { openInBrowser, useWebViews } from '@/lib/web-views'
import type { AttentionItem } from '@/rpc/bindings/method.roster.output'
import { TONE_ACCENT, TONE_NEUTRAL } from './actions'
import { Footer, useResolved } from './shared'
import { ActionBar, Source } from './source'
import { webViewOf } from './model'
import { setItemError } from './queries'

/**
 * A `web_view` item (205): Source · the page's title and address · the live page filling the pane · Open in chat · Pop
 * out · Copy URL · Snooze · Dismiss; the tile's address bar (Back / Forward / Reload / address / Open in browser) sits above the page. The page is the chat's own frame (`web-<id>`): showing it
 * here moves its native view over this pane, leaving the pane (another item, another page) hides it again, and the chat's
 * tile takes it back when it shows. Without a native layer (a browser, the dev fixture) the pane shows the address with
 * **Open in browser** (205 D7), the same size as the page would be.
 */
export function WebViewScreen({ item }: { item: AttentionItem }) {
  const page = webViewOf(item)
  const onResolved = useResolved(item)
  const navigate = useNavigate()
  const mobile = useIsMobile()
  // The frame may be unknown to this window (it started after the `show_web`): record it, as the chat card's Open does.
  const known = useWebViews((s) => s.views[page.id] !== undefined)
  useEffect(() => {
    if (!known && page.session) useWebViews.getState().open({ id: page.id, session: page.session, url: page.url, title: page.title })
  }, [known, page.id, page.session, page.url, page.title])
  const { view, current, live, remote, engine, force } = useLiveFrame(page.id)
  // A pop-out window holds the page (205 D6): it stays there; this pane says so instead of taking it back.
  const [elsewhere, setElsewhere] = useState(false)
  const popped = elsewhere || view?.popped === true
  const ref = useRef<HTMLDivElement>(null)
  const shows = live && !popped
  const occluded = useNativeLayer(page.id, ref, shows, (e) => (String(e).startsWith('in-popout:') ? setElsewhere(true) : warn('web_view_attach')(e)))
  const url = current || page.url
  const title = view?.title || page.title

  const popOut = () => void popOutWeb({ id: page.id, session: view?.session || page.session, url, title }).then(() => setElsewhere(true), warn('open_window'))
  const openInChat = () => {
    if (!page.session) return
    const session = page.session
    // The chat's pane must be in the tree before the tile is placed beside it.
    void navigate({ to: '/c/$session', params: { session } }).then(() => requestAnimationFrame(() => openWebView(session, { id: page.id, url, title }, mobile && native.nativeWebViews())))
  }
  // The inbox never toasts: the button itself says Copied for a moment; a failure shows under the item.
  const [copied, setCopied] = useState(false)
  useEffect(() => {
    if (!copied) return
    const t = setTimeout(() => setCopied(false), 1500)
    return () => clearTimeout(t)
  }, [copied])
  const copy = () =>
    void navigator.clipboard.writeText(url).then(
      () => {
        setItemError([item.id], null)
        setCopied(true)
      },
      () => setItemError([item.id], 'Copy failed'),
    )

  return (
    <>
      <div className="shrink-0 border-b border-border-subtle px-4 py-3 md:px-6">
        <div className="mx-auto flex w-full max-w-[720px] flex-col gap-2">
          <Source item={item} />
          <h1 className="truncate text-lg font-semibold leading-snug">{title}</h1>
          {/* The tile's own address bar (205 §4.4): Back · Forward · Reload · the address field · Open in browser. */}
          <div className="flex items-center gap-1">
            <Button variant="ghost" size="icon" className="size-8 shrink-0 rounded-lg text-fg-secondary" aria-label="Back" disabled={!shows} onClick={() => void native.history(page.id, -1).catch(warn('web_view_history'))}>
              <ArrowLeft className="size-4" />
            </Button>
            <Button variant="ghost" size="icon" className="size-8 shrink-0 rounded-lg text-fg-secondary" aria-label="Forward" disabled={!shows} onClick={() => void native.history(page.id, 1).catch(warn('web_view_history'))}>
              <ArrowRight className="size-4" />
            </Button>
            <Button variant="ghost" size="icon" className="size-8 shrink-0 rounded-lg text-fg-secondary" aria-label="Reload" disabled={!shows} onClick={() => void native.reload(page.id).catch(warn('web_view_reload'))}>
              <RotateCw className={cn('size-4', view?.loading && 'animate-spin')} />
            </Button>
            <AddressField id={page.id} live={shows} current={url} compact={mobile} className="mx-1 h-8 text-xs" />
            <Button variant="ghost" size="icon" className="size-8 shrink-0 rounded-lg text-fg-secondary" aria-label="Open in browser" disabled={!url} onClick={() => openInBrowser(url)}>
              <ExternalLink className="size-4" />
            </Button>
          </div>
        </div>
      </div>
      <div ref={ref} data-web-frame={page.id} className={cn('relative min-h-0 flex-1 bg-muted/40')}>
        {popped ? (
          <Quiet title={title} url={url}>
            <p className="text-sm text-fg-secondary">This page is open in its own window.</p>
            {canPopout() && (
              <Button variant="outline" size="sm" onClick={popOut}>
                <AppWindow />
                Show window
              </Button>
            )}
          </Quiet>
        ) : (
          <FrameFace live={live} occluded={occluded} remote={remote} engine={engine} title={title} url={url} onForce={force} />
        )}
      </div>
      <ActionBar>
        {page.session && (
          <Button size="sm" className={TONE_ACCENT} onClick={openInChat}>
            <MessageSquare />
            Open in chat
          </Button>
        )}
        {canPopout() && (
          <Button size="sm" variant="outline" className={TONE_NEUTRAL} onClick={popOut}>
            <AppWindow />
            Pop out
          </Button>
        )}
        <Button size="sm" variant="ghost" className={TONE_NEUTRAL} disabled={!url} onClick={copy}>
          {copied ? <Check /> : <Copy />}
          {copied ? 'Copied' : 'Copy URL'}
        </Button>
        <Footer item={item} onResolved={onResolved} />
      </ActionBar>
    </>
  )
}
