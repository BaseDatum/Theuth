import { ChevronDown, FileCode } from 'lucide-react'
import { useState } from 'react'
import { FileView, useFileFollow } from '@/components/code/FileView'
import { openFileTile } from '@/components/tiles/open'
import { useTileHost } from '@/components/tiles/host'
import { Button } from '@/components/ui/button'
import { cn } from '@/lib/utils'
import type { Entry } from '@/store/sessions'
import { ToolSpinner } from './Indicators'
import { OpenLabel, openLabel, useHostTileOpen } from './ViewOpenedCard'
import { PopoutButton } from './PopoutButton'

type Tool = Extract<Entry, { kind: 'tool' }>
/** `id`: the descriptor id from the receipt, `null` while pending or failed. `path` is absolute once received. */
export type FileCall = { title: string; id: string | null; path: string; line?: number; end_line?: number }

/** The receipt's first two lines (212 §1): `file view opened · <title> · tile file:<id>`, then `path: <abs>` with
 * `#L<line>` or `#L<line>-L<end>` when the call named lines. The arguments stand in while pending. */
const RECEIPT = /^file view opened · (.*) · tile file:([0-9a-f]{16})$/m
const PATH = /^path: (.+?)(?:#L(\d+)(?:-L(\d+))?)?$/m

const lines = (line: unknown, end: unknown) => ({
  ...(typeof line === 'number' || typeof line === 'string' ? { line: Number(line) } : {}),
  ...(typeof end === 'number' || typeof end === 'string' ? { end_line: Number(end) } : {}),
})

export function fileCall(tool: Pick<Tool, 'args' | 'result'>): FileCall {
  const output = tool.result && !tool.result.isError ? tool.result.output : ''
  const m = RECEIPT.exec(output)
  const p = m && PATH.exec(output)
  if (m && p) return { title: m[1] as string, id: m[2] as string, path: p[1] as string, ...lines(p[2], p[3]) }
  const args = (tool.args ?? {}) as { path?: unknown; title?: unknown; line?: unknown; end_line?: unknown }
  const path = typeof args.path === 'string' ? args.path : ''
  const title = typeof args.title === 'string' && args.title ? args.title : (path.split('/').pop() ?? '')
  return { title, id: null, path, ...lines(args.line, args.end_line) }
}

/** The card's inline file (no tile here), following the disk while unfolded. */
function Inline({ session, call }: { session: string; call: FileCall }) {
  const { deleted } = useFileFollow(session, call.path)
  return (
    <div className="mt-2 overflow-hidden rounded-md border">
      {deleted && <div className="border-b bg-layer-2 px-3 py-1 font-mono text-xs text-fg-tertiary">deleted</div>}
      <FileView session={session} path={call.path} line={call.line} end={call.end_line} className="max-h-96" />
    </div>
  )
}

/** The transcript card of a `show_file` call (212 §4): title, path and lines; **Open** puts the file tile beside the
 * chat in leaf `host`, and with no tile there (a phone, no pane) unfolds the file in the card. */
export function FileViewCard({ tool, session }: { tool: Tool; session: string }) {
  const host = useTileHost()
  const [inline, setInline] = useState(false)
  const call = fileCall(tool)
  const shown = useHostTileOpen(host, 'file', call.id ?? '')
  const failed = tool.result?.isError === true
  const running = !tool.interrupted && (tool.pending || !tool.result)
  const id = call.id
  const open = (id: string) => {
    if (host && openFileTile(host, { session, path: call.path, id, title: call.title, ...lines(call.line, call.end_line) }) !== null) return
    setInline((v) => !v)
  }
  const range = call.line === undefined ? '' : call.end_line !== undefined && call.end_line !== call.line ? `:${call.line}-${call.end_line}` : `:${call.line}`
  return (
    <div className="my-2 min-w-0 rounded-xl border border-border-subtle bg-layer-1 px-4 py-3 text-sm" data-file-card={id ?? undefined}>
      <div className="flex min-w-0 items-center gap-3">
        <FileCode className="size-5 shrink-0 text-fg-secondary" />
        <div className="flex min-w-0 flex-1 flex-col">
          <span className="truncate font-medium">{call.title || 'File'}</span>
          {call.path && <span className="truncate font-mono text-xs text-fg-secondary" title={call.path}>{call.path}{range}</span>}
        </div>
        {running && <ToolSpinner />}
        {id && (
          <Button variant="outline" size="sm" aria-expanded={host ? undefined : inline} onClick={() => open(id)}>
            {host ? (
              <OpenLabel label={openLabel(true, shown)} />
            ) : (
              <>
                <ChevronDown className={cn(inline && 'rotate-180')} />
                Open
              </>
            )}
          </Button>
        )}
        {id && call.path && <PopoutButton target={{ kind: 'file', session, path: call.path, ...lines(call.line, call.end_line) }} />}
      </div>
      {id && inline && <Inline session={session} call={call} />}
      {failed && <p className="mt-2 text-xs text-destructive wrap-anywhere">{tool.result?.output}</p>}
    </div>
  )
}
