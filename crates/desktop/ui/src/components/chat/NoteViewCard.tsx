import { useNavigate } from '@tanstack/react-router'
import { AppWindow, FileText, PanelRight } from 'lucide-react'
import { toast } from 'sonner'
import { type NoteProps, openNoteTile } from '@/components/tiles/open'
import { useTileHost } from '@/components/tiles/host'
import { Button } from '@/components/ui/button'
import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip'
import { canPopout, openPopout } from '@/lib/popout'
import type { Entry } from '@/store/sessions'
import { ToolSpinner } from './Indicators'

type Tool = Extract<Entry, { kind: 'tool' }>
export type NoteCall = { title: string; doc: string | null; block?: string }

/** The receipt's first line `note view opened · <title> · tile note:<doc>` (211 §1); its second line,
 * `theuth://notes/<doc>#<block>`, carries the block when the call named one. The arguments stand in while pending. */
const RECEIPT = /^note view opened · (.*) · tile note:([0-9a-z]{26})$/m
const BLOCK = /^theuth:\/\/notes\/[0-9a-z]{26}#(\S+)$/m

export function noteCall(tool: Pick<Tool, 'args' | 'result'>): NoteCall {
  const output = tool.result && !tool.result.isError ? tool.result.output : ''
  const m = RECEIPT.exec(output)
  if (m) {
    const block = BLOCK.exec(output)?.[1]
    return { title: m[1] as string, doc: m[2] as string, ...(block ? { block } : {}) }
  }
  const args = (tool.args ?? {}) as { doc?: unknown; title?: unknown }
  const doc = typeof args.doc === 'string' ? args.doc : ''
  return { title: typeof args.title === 'string' && args.title ? args.title : doc, doc: null }
}

/** The transcript card of a `show_note` call (211 §3): title, **Open** (the document's note tile beside the chat in
 * leaf `host`; with no tile there the `/notes` page), pop out where windows exist. */
export function NoteViewCard({ tool }: { tool: Tool }) {
  const host = useTileHost()
  const navigate = useNavigate()
  const call = noteCall(tool)
  const failed = tool.result?.isError === true
  const running = !tool.interrupted && (tool.pending || !tool.result)
  const doc = call.doc
  const open = (note: NoteProps) => {
    if (host && openNoteTile(host, note) !== null) return
    void navigate({ to: '/notes/$id', params: { id: note.doc }, hash: note.block })
  }
  return (
    <div className="my-2 min-w-0 rounded-xl border border-border-subtle bg-layer-1 px-4 py-3 text-sm" data-note-card={doc ?? undefined}>
      <div className="flex min-w-0 items-center gap-3">
        <FileText className="size-5 shrink-0 text-fg-secondary" />
        <span className="min-w-0 flex-1 truncate font-medium">{call.title || 'Note'}</span>
        {running && <ToolSpinner />}
        {doc && (
          <Button variant="outline" size="sm" onClick={() => open({ doc, title: call.title, ...(call.block ? { block: call.block } : {}) })}>
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
                onClick={() => void openPopout({ kind: 'note', doc }).catch((e: unknown) => toast.error(String(e)))}
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
