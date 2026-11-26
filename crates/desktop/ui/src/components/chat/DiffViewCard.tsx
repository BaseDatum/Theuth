import { ChevronDown, FileDiff } from 'lucide-react'
import { useState } from 'react'
import { DiffBody, DiffLayoutToggle, INLINE_ROW_CAP, useDiffLayout } from '@/components/code/DiffBody'
import { openDiffTile } from '@/components/tiles/open'
import { useTileHost } from '@/components/tiles/host'
import { Button } from '@/components/ui/button'
import { cn } from '@/lib/utils'
import type { DiffSpec } from '@/rpc/bindings/method.artifact.input'
import type { Entry } from '@/store/sessions'
import { DiffStats } from './DiffView'
import { ToolSpinner } from './Indicators'
import { OpenLabel, openLabel, useHostTileOpen } from './ViewOpenedCard'
import { PopoutButton } from './PopoutButton'

type Tool = Extract<Entry, { kind: 'tool' }>
/** `id` and `spec` come from the receipt (`null` while pending or failed); `stats` from its counts line, `none` for
 * `no changes.`. */
export type DiffCall = { title: string; id: string | null; spec: DiffSpec | null; stats: { files: number; added: number; removed: number } | 'none' | null }

/** The receipt's first two lines (212 §1): `diff view opened · <title> · tile diff:<id>`, then `spec: <JSON>`; the
 * third is `N files changed, +A −R.` or `no changes.`. The arguments name the call while pending. */
const RECEIPT = /^diff view opened · (.*) · tile diff:([0-9a-f]{16})$/m
const SPEC = /^spec: (\{.*\})$/m
const COUNTS = /^(\d+) files? changed, \+(\d+) −(\d+)\.$/m

function specOf(output: string): DiffSpec | null {
  const s = SPEC.exec(output)
  if (!s) return null
  try {
    return JSON.parse(s[1] as string) as DiffSpec
  } catch {
    return null
  }
}

/** What a pending call will be titled, from its arguments (the tool's own defaults, roughly). */
function argsTitle(args: Record<string, unknown>): string {
  const str = (k: string) => (typeof args[k] === 'string' && args[k] !== '' ? (args[k] as string) : undefined)
  const base = (p: string) => p.split('/').pop() ?? p
  const fa = str('file_a'), fb = str('file_b')
  return str('title') ?? str('commit') ?? str('range') ?? (args.staged === true ? 'staged' : fa && fb ? `${base(fa)} ↔ ${base(fb)}` : 'working tree')
}

export function diffCall(tool: Pick<Tool, 'args' | 'result'>): DiffCall {
  const output = tool.result && !tool.result.isError ? tool.result.output : ''
  const m = RECEIPT.exec(output)
  const spec = m && specOf(output)
  if (m && spec) {
    const c = COUNTS.exec(output)
    const stats = c ? { files: Number(c[1]), added: Number(c[2]), removed: Number(c[3]) } : /^no changes\.$/m.test(output) ? 'none' : null
    return { title: m[1] as string, id: m[2] as string, spec, stats }
  }
  return { title: argsTitle((tool.args ?? {}) as Record<string, unknown>), id: null, spec: null, stats: null }
}

/** The transcript card of a `show_diff` call (212 §4): title, `+A −R`; **Open** puts the diff tile beside the chat in
 * leaf `host`, and with no tile there (a phone, no pane) unfolds the diff in the card. */
export function DiffViewCard({ tool, session }: { tool: Tool; session: string }) {
  const host = useTileHost()
  const [inline, setInline] = useState(false)
  const call = diffCall(tool)
  const shown = useHostTileOpen(host, 'diff', call.id ?? '')
  const failed = tool.result?.isError === true
  const running = !tool.interrupted && (tool.pending || !tool.result)
  const { id, spec, stats } = call
  const open = (id: string, spec: DiffSpec) => {
    if (host && openDiffTile(host, { session, id, title: call.title, spec }) !== null) return
    setInline((v) => !v)
  }
  return (
    <div className="my-2 min-w-0 rounded-xl border border-border-subtle bg-layer-1 px-4 py-3 text-sm" data-diff-card={id ?? undefined}>
      <div className="flex min-w-0 items-center gap-3">
        <FileDiff className="size-5 shrink-0 text-fg-secondary" />
        <div className="flex min-w-0 flex-1 flex-col">
          <span className="truncate font-medium">{call.title || 'Diff'}</span>
          {stats && (
            <span className="flex items-center gap-2 font-mono text-xs text-fg-secondary">
              {stats === 'none' ? 'no changes' : <>{stats.files} {stats.files === 1 ? 'file' : 'files'} <DiffStats added={stats.added} removed={stats.removed} /></>}
            </span>
          )}
        </div>
        {running && <ToolSpinner />}
        {id && spec && (
          <Button variant="outline" size="sm" aria-expanded={host ? undefined : inline} onClick={() => open(id, spec)}>
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
        {id && spec && <PopoutButton target={{ kind: 'diff', session, spec }} />}
      </div>
      {spec && inline && <Inline session={session} spec={spec} />}
      {failed && <p className="mt-2 text-xs text-destructive wrap-anywhere">{tool.result?.output}</p>}
    </div>
  )
}

/** The card's inline diff (no tile here), Unified | Split above it; Split by default only when the card is wide. */
function Inline({ session, spec }: { session: string; spec: DiffSpec }) {
  const { layout, choose, ref } = useDiffLayout()
  return (
    <div ref={ref} className="mt-2 flex flex-col gap-2">
      <DiffLayoutToggle layout={layout} onChange={choose} className="self-end" />
      <div className="overflow-hidden rounded-md border">
        <DiffBody session={session} spec={spec} cap={INLINE_ROW_CAP} layout={layout} className="max-h-96" />
      </div>
    </div>
  )
}
