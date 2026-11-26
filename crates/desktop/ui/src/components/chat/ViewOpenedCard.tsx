import { useNavigate } from '@tanstack/react-router'
import { Eye, LayoutPanelLeft, PanelRight, RotateCcw } from 'lucide-react'
import { openViewFromCard, useTileOpen } from '@/components/tiles/open'
import { useTileHost } from '@/components/tiles/host'
import { Button } from '@/components/ui/button'
import { isTile, ownerOf, type PanePlacement, tileLeaf, usePanes } from '@/lib/panes'
import { tileId } from '@/lib/tiles'
import type { Entry } from '@/store/sessions'
import { ToolSpinner } from './Indicators'
import { PopoutButton } from './PopoutButton'

type Tool = Extract<Entry, { kind: 'tool' }>
/** `leaf`: the pane the receipt named, `null` while pending, failed or unparsable. */
export type ViewCall = { title: string; path: string; leaf: string | null; reused: boolean }

/** `show_view`'s pinned first line, `view opened · <title> · <path> · tile <leaf>` (`view reused · …`), written by
 * `view_receipt` in `crates/artifact/src/layout.rs`; receipts written before it carry only the human line
 * `Opened|Reused <title> (<path>) in pane <leaf>`. */
const RECEIPT = /^view (opened|reused) · (.*) · (\/\S*) · tile (\S+)$/m
const LEGACY = /^(Opened|Reused) (.*) \((\/\S*)\) in pane (\S+)$/m

export function viewCall(tool: Pick<Tool, 'args' | 'result'>): ViewCall {
  const output = tool.result && !tool.result.isError ? tool.result.output : ''
  const m = RECEIPT.exec(output) ?? LEGACY.exec(output)
  if (m) return { title: m[2] as string, path: m[3] as string, leaf: m[4] as string, reused: m[1]?.toLowerCase() === 'reused' }
  const args = viewArgs(tool.args)
  return { title: args.title ?? args.path, path: args.path, leaf: null, reused: false }
}

/** The call's own `path`, `title`, `placement` and `target`: what Reopen asks for again. */
function viewArgs(raw: unknown): { path: string; title?: string; placement?: PanePlacement; target?: string } {
  const a = (raw ?? {}) as Record<string, unknown>
  const str = (k: string) => (typeof a[k] === 'string' && a[k] !== '' ? (a[k] as string) : undefined)
  const placement = str('placement') as PanePlacement | undefined
  const target = str('target')
  const title = str('title')
  return { path: str('path') ?? '', ...(title ? { title } : {}), ...(placement ? { placement } : {}), ...(target ? { target } : {}) }
}

/** A card's open button label: `Focus` while its tile is laid out, `Reopen` once closed; `Open` where nothing tiles. */
export function openLabel(tiles: boolean, open: boolean): 'Focus' | 'Reopen' | 'Open' {
  return !tiles ? 'Open' : open ? 'Focus' : 'Reopen'
}

/** Reactive: tile `kind` (with `key`) of the chat shown in leaf `host` is laid out — the id `openTile` gives it. */
export function useHostTileOpen(host: string | null, kind: string, key?: string): boolean {
  return usePanes((s) => {
    if (!host) return false
    const session = isTile(host) ? ownerOf(host, s.views[host]) || host : host
    return s.leaves.includes(tileLeaf(tileId(kind, session, key)))
  })
}

/** The card's open button: the label's icon and text. */
export function OpenLabel({ label }: { label: 'Focus' | 'Reopen' | 'Open' }) {
  return (
    <>
      {label === 'Focus' ? <Eye /> : label === 'Reopen' ? <RotateCcw /> : <PanelRight />}
      {label}
    </>
  )
}

/**
 * The transcript card of a `show_view` call: the view's title and route, **Focus** while its pane is open, **Reopen**
 * once it was closed (with the call's own placement and target, through `openViewFromCard`, which never closes another
 * pane), and pop out where windows exist. On a phone, Open navigates to the route.
 */
export function ViewOpenedCard({ tool, session }: { tool: Tool; session: string }) {
  const host = useTileHost()
  const navigate = useNavigate()
  const call = viewCall(tool)
  const failed = tool.result?.isError === true
  const running = !tool.interrupted && (tool.pending || !tool.result)
  const open = useTileOpen(call.leaf ?? call.path)
  const ready = call.leaf !== null && call.path !== ''
  const reopen = () => {
    const { placement, target, title = call.title } = viewArgs(tool.args)
    openViewFromCard({ path: call.path, ...(title ? { title } : {}) }, { placement, target, session, navigate: (href) => void navigate({ href }) })
  }
  return (
    <div className="my-2 min-w-0 rounded-xl border border-border-subtle bg-layer-1 px-4 py-3 text-sm" data-view-card={call.leaf ?? undefined}>
      <div className="flex min-w-0 items-center gap-3">
        <LayoutPanelLeft className="size-5 shrink-0 text-fg-secondary" />
        <div className="flex min-w-0 flex-1 flex-col">
          <span className="truncate font-medium">{call.title || 'View'}</span>
          {call.path && <span className="truncate font-mono text-xs text-fg-secondary" title={call.path}>{call.path}</span>}
        </div>
        {running && <ToolSpinner />}
        {ready && (
          <Button variant="outline" size="sm" onClick={reopen}>
            <OpenLabel label={openLabel(host !== null, open)} />
          </Button>
        )}
        {ready && <PopoutButton target={{ kind: 'page', path: call.path }} />}
      </div>
      {failed && <p className="mt-2 text-xs text-destructive wrap-anywhere">{tool.result?.output}</p>}
    </div>
  )
}
