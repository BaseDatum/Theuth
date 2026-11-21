import { AppWindow, ExternalLink, Globe, PanelRight } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { useSidebar } from '@/components/ui/sidebar'
import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip'
import { placeWeb, usePanes } from '@/lib/panes'
import { openExternal } from '@/lib/platform'
import { canPopout, openPopout } from '@/lib/popout'
import { useWebViews, webLeaf } from '@/lib/web-views'
import type { Entry } from '@/store/sessions'
import { ToolSpinner } from './Indicators'

type Tool = Extract<Entry, { kind: 'tool' }>
export type WebCall = { id: string | null; url: string; title: string }

/** The receipt `web view opened · <title> · <url> · tile web:<id>` (205 §4.1). The transcript keeps a call's content,
 * not its `structured` value, so the id comes from here; the arguments stand in while the call is pending. */
const RECEIPT = /^web view opened · (.*) · (\S+) · tile web:([0-9a-f]{16})/

export function webCall(tool: Pick<Tool, 'args' | 'result'>): WebCall {
  const m = tool.result && !tool.result.isError ? RECEIPT.exec(tool.result.output) : null
  if (m) return { title: m[1] as string, url: m[2] as string, id: m[3] as string }
  const args = (tool.args ?? {}) as { url?: unknown; title?: unknown }
  const url = typeof args.url === 'string' ? args.url : ''
  return { id: null, url, title: typeof args.title === 'string' && args.title ? args.title : url }
}

/** **Open**: the frame's tile right of `session`'s pane (an open one stays), or its popped-out window, focused. */
export function openWebView(session: string, { id, url, title }: { id: string; url: string; title: string }): void {
  const store = useWebViews.getState()
  const view = store.views[id]
  if (!view) store.open({ id, session, url, title })
  if (view?.popped && canPopout()) {
    void openPopout({ kind: 'web', id, url: view.current, title: view.title })
    return
  }
  store.patch(id, { popped: false })
  placeWeb(session, id)
}

/** The transcript card of a `show_web` call (205 §4.4): title, address, Open, pop out, open in browser. A phone does not
 * tile, so there **Open in browser** is the card's action. */
export function WebViewCard({ tool, session }: { tool: Tool; session: string }) {
  const { isMobile } = useSidebar()
  const call = webCall(tool)
  const failed = tool.result?.isError === true
  const running = !tool.interrupted && (tool.pending || !tool.result)
  const id = call.id
  return (
    <div className="my-2 min-w-0 rounded-xl border border-border-subtle bg-layer-1 px-4 py-3 text-sm" data-web-card={id ?? undefined}>
      <div className="flex min-w-0 items-center gap-3">
        <Globe className="size-5 shrink-0 text-fg-secondary" />
        <div className="flex min-w-0 flex-1 flex-col">
          <span className="truncate font-medium">{call.title || 'Web page'}</span>
          <span className="truncate font-mono text-xs text-fg-secondary">{call.url}</span>
        </div>
        {running && <ToolSpinner />}
        {id && !isMobile && (
          <Button variant="outline" size="sm" onClick={() => openWebView(session, { id, url: call.url, title: call.title })}>
            <PanelRight />
            Open
          </Button>
        )}
        {call.url && !failed && (
          <Button variant={isMobile ? 'outline' : 'ghost'} size={isMobile ? 'sm' : 'icon-sm'} className={isMobile ? undefined : 'text-fg-secondary'} aria-label="Open in browser" onClick={() => openExternal(call.url)}>
            <ExternalLink className={isMobile ? undefined : 'size-4'} />
            {isMobile && 'Open in browser'}
          </Button>
        )}
        {id && canPopout() && (
          <Tooltip>
            <TooltipTrigger asChild>
              <Button
                variant="ghost"
                size="icon-sm"
                className="text-fg-secondary"
                aria-label="Open in new window"
                onClick={() => {
                  // 205 D6: the window takes the page; a tile of it here closes.
                  useWebViews.getState().patch(id, { popped: true })
                  void openPopout({ kind: 'web', id, url: useWebViews.getState().views[id]?.current ?? call.url, title: call.title })
                  usePanes.getState().close(webLeaf(id))
                }}
              >
                <AppWindow className="size-4" />
              </Button>
            </TooltipTrigger>
            <TooltipContent>Open in new window</TooltipContent>
          </Tooltip>
        )}
      </div>
      {failed && <p className="mt-2 text-xs text-destructive wrap-anywhere">{tool.result?.output}</p>}
    </div>
  )
}
