import { useNavigate } from '@tanstack/react-router'
import { AppWindow, PanelRight, Presentation } from 'lucide-react'
import { notify as toast } from '@/lib/notify'
import { openWhiteboardTile, type WhiteboardProps } from '@/components/tiles/open'
import { useTileHost } from '@/components/tiles/host'
import { Button } from '@/components/ui/button'
import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip'
import { canPopout, openPopout } from '@/lib/popout'
import type { Entry } from '@/store/sessions'
import { ToolSpinner } from './Indicators'

type Tool = Extract<Entry, { kind: 'tool' }>
/** `cards`: the count the receipt carried when the call ran (a summary, not live). */
export type WhiteboardCall = { title: string; doc: string | null; cards?: number }

/** `show_whiteboard`'s receipt: `opened whiteboard "<title>"` then `theuth://whiteboards/<doc>` (218 §3.4). */
const SHOWN = /^opened whiteboard (".*")$/m
const LINK = /^theuth:\/\/whiteboards\/([0-9a-z]{26})$/m
/** `wb_create`'s receipt: `created whiteboard "<title>" <doc> — show it with …`. */
const CREATED = /^created whiteboard (".*") ([0-9a-z]{26}) — /m
/** Both receipts' summary line, `N cards` (absent from receipts written before it existed). */
const CARDS = /^(\d+) cards?$/m

const unquote = (s: string): string => {
  try {
    const v: unknown = JSON.parse(s)
    return typeof v === 'string' ? v : s
  } catch {
    return s.slice(1, -1)
  }
}

/** The whiteboard a `show_whiteboard` or `wb_create` call opened or made, read from its receipt (the transcript keeps
 * it across a reload); the arguments stand in (with no document) while it runs or when it failed. */
export function whiteboardCall(tool: Pick<Tool, 'name' | 'args' | 'result'>): WhiteboardCall {
  const output = tool.result && !tool.result.isError ? tool.result.output : ''
  const n = CARDS.exec(output)
  const cards = n ? { cards: Number(n[1]) } : {}
  const created = CREATED.exec(output)
  if (created) return { title: unquote(created[1] as string), doc: created[2] as string, ...cards }
  const shown = SHOWN.exec(output)
  const link = LINK.exec(output)
  if (shown && link) return { title: unquote(shown[1] as string), doc: link[1] as string, ...cards }
  const args = (tool.args ?? {}) as { title?: unknown; whiteboard?: unknown }
  const title = typeof args.title === 'string' && args.title ? args.title : typeof args.whiteboard === 'string' ? args.whiteboard : ''
  return { title, doc: null }
}

/** `created · 0 cards`, `3 cards`: the card's second line. */
function summary(tool: Pick<Tool, 'name'>, call: WhiteboardCall): string {
  const count = call.cards === undefined ? null : call.cards === 0 ? 'empty' : `${call.cards} ${call.cards === 1 ? 'card' : 'cards'}`
  return [tool.name === 'wb_create' ? 'New whiteboard' : 'Whiteboard', count].filter(Boolean).join(' · ')
}

/** Opens whiteboard `doc`: its tile beside the chat in leaf `host`; with no tile there (a phone, the iPad's full-window
 * whiteboard) its own page, bound to `session`. */
export function useOpenWhiteboard(session: string): (props: WhiteboardProps) => void {
  const host = useTileHost()
  const navigate = useNavigate()
  return (props) => {
    if (host && openWhiteboardTile(host, props) !== null) return
    void navigate({ to: '/whiteboards/$id', params: { id: props.doc }, search: { session } })
  }
}

/** The transcript card of a `show_whiteboard` or `wb_create` call: title, summary, **Open**, pop out where windows exist. */
export function WhiteboardViewCard({ tool, session }: { tool: Tool; session: string }) {
  const open = useOpenWhiteboard(session)
  const call = whiteboardCall(tool)
  const failed = tool.result?.isError === true
  const running = !tool.interrupted && (tool.pending || !tool.result)
  const doc = call.doc
  const props = (doc: string): WhiteboardProps => ({ doc, ...(call.title ? { title: call.title } : {}) })
  return (
    <div className="my-2 min-w-0 rounded-xl border border-border-subtle bg-layer-1 px-4 py-3 text-sm" data-whiteboard-card={doc ?? undefined}>
      <div className="flex min-w-0 items-center gap-3">
        <Presentation className="size-5 shrink-0 text-fg-secondary" />
        <div className="flex min-w-0 flex-1 flex-col">
          <span className="truncate font-medium">{call.title || 'Whiteboard'}</span>
          {doc && <span className="truncate text-xs text-fg-tertiary">{summary(tool, call)}</span>}
        </div>
        {running && <ToolSpinner />}
        {doc && (
          <Button variant="outline" size="sm" onClick={() => open(props(doc))}>
            <PanelRight />
            Open
          </Button>
        )}
        {doc && canPopout() && (
          <Tooltip>
            <TooltipTrigger asChild>
              <Button
                variant="ghost"
                size="icon-sm"
                className="text-fg-secondary"
                aria-label="Open in new window"
                onClick={() => void openPopout({ kind: 'tile', tile: 'whiteboard', session, props: props(doc) }).catch((e: unknown) => toast.error(String(e)))}
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
