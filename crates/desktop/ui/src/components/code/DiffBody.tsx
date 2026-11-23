// 212 §4: the diff renderer shared by an edit card's `DiffView` (capped, inside the card) and the `show_diff` tile and
// card (`DiffBody`, every file, collapsible, fetched with `diff_view_read`).
import { useQuery } from '@tanstack/react-query'
import { ChevronRight } from 'lucide-react'
import { useMemo, useState } from 'react'
import { type DiffFile, type DiffRow, type WordPart, parseUnified, wordChanges } from '@/lib/editDiff'
import { queryKeys, useApi } from '@/lib/rpc'
import { cn } from '@/lib/utils'
import type { DiffSpec } from '@/rpc/bindings/method.artifact.input'
import type { DiffFileStat, DiffViewText } from '@/rpc/bindings/method.artifact.output'
import { Unavailable, useQuerySupport } from '@/rpc/capabilities'

const ROW = { ' ': 'text-fg-secondary', '-': 'bg-danger-bg', '+': 'bg-success-bg' }

/** Pair adjacent removal/addition runs, without crossing context or hunk boundaries. */
function marksOf(rows: DiffRow[]): Map<number, WordPart[]> {
  const marks = new Map<number, WordPart[]>()
  for (let i = 0; i < rows.length; i++) {
    if (rows[i]?.op !== '-') continue
    const removed: number[] = [], added: number[] = []
    while (rows[i]?.op === '-' || rows[i]?.kind === 'note') { if (rows[i]?.op === '-') removed.push(i); i++ }
    while (rows[i]?.op === '+' || rows[i]?.kind === 'note') { if (rows[i]?.op === '+') added.push(i); i++ }
    for (let n = 0; n < Math.min(removed.length, added.length); n++) {
      const a = removed[n]!, b = added[n]!
      const [before, after] = wordChanges(rows[a]!.text, rows[b]!.text)
      marks.set(a, before); marks.set(b, after)
    }
    i--
  }
  return marks
}

/** One file's hunk headers, notes and numbered lines, changed words marked. */
function DiffTable({ rows }: { rows: DiffRow[] }) {
  const marks = marksOf(rows)
  return <table className="w-full border-collapse"><tbody>
    {rows.map((row, i) => row.kind !== 'line' ? <tr key={i}>
      <td colSpan={4} className={cn('px-3 whitespace-pre text-fg-tertiary', row.kind === 'hunk' && 'bg-project-slate/10')}>{row.text}</td>
    </tr> : <tr key={i} className={ROW[row.op ?? ' ']}>
      <td className="w-px min-w-9 select-none px-1.5 text-right align-top text-fg-tertiary">{row.old}</td>
      <td className="w-px min-w-9 select-none px-1.5 text-right align-top text-fg-tertiary">{row.next}</td>
      <td className={cn('w-px select-none px-1 align-top', row.op === '+' ? 'text-success' : row.op === '-' ? 'text-danger' : '')}>{row.op === '-' ? '−' : row.op}</td>
      <td className="w-full pr-3 whitespace-pre">{marks.get(i)?.map((part, n) => !part.changed ? part.text : row.op === '+'
        ? <ins key={n} className="rounded-sm bg-success/25 no-underline">{part.text}</ins>
        : <del key={n} className="rounded-sm bg-danger/25 no-underline">{part.text}</del>) ?? row.text}</td>
    </tr>)}
  </tbody></table>
}

export type FileStat = Pick<DiffFileStat, 'status' | 'added' | 'removed'>

/**
 * Files of a parsed patch, each a section under a sticky header, drawing at most `cap` rows across all files (then
 * Show all, when `onShowAll` is given). The caller's `className` is the chrome (an edit card's capped strip, a tile's
 * full height). With `stats` (the `diff_view_read` per-file counts, by path) a header is a fold button carrying the
 * file's status and `+N −M` instead of the hunk count.
 */
export function DiffSections({ files, cap = Infinity, onShowAll, className, stats }: {
  files: DiffFile[]
  cap?: number
  onShowAll?: () => void
  className?: string
  stats?: Map<string, FileStat>
}) {
  const [folded, setFolded] = useState<ReadonlySet<number>>(() => new Set())
  const fold = (f: number) => setFolded((s) => {
    const next = new Set(s)
    if (!next.delete(f)) next.add(f)
    return next
  })
  const total = files.reduce((n, file) => n + file.rows.length, 0)
  let remaining = cap
  return <div className={className} aria-label="File diff">
    {files.map((file, f) => {
      if (remaining <= 0) return null
      const shut = folded.has(f)
      const visible = shut ? [] : file.rows.slice(0, remaining)
      remaining -= visible.length
      const path = file.path || 'Edited file'
      const stat = stats?.get(file.path)
      const hunks = file.rows.reduce((n, row) => n + Number(row.kind === 'hunk'), 0)
      return <section key={f} aria-label={path}>
        {stats ? (
          <button type="button" aria-expanded={!shut} onClick={() => fold(f)} className="sticky top-0 z-10 flex w-full items-center gap-2 border-b border-border-subtle bg-layer-1 px-2 py-1 text-left text-fg-secondary hover:text-foreground">
            <ChevronRight className={cn('size-3.5 shrink-0 transition-transform', !shut && 'rotate-90')} />
            <span className="min-w-0 flex-1 break-all">{path}</span>
            {stat && <span className="shrink-0 text-fg-tertiary">{stat.status}</span>}
            {stat && <span className="inline-flex shrink-0 gap-2"><span className="text-success">+{stat.added}</span><span className="text-danger">−{stat.removed}</span></span>}
          </button>
        ) : (
          <div className="sticky top-0 z-10 flex gap-3 border-b border-border-subtle bg-layer-1 px-3 py-1 text-fg-secondary">
            <span className="break-all">{path}</span>
            <span className="shrink-0 text-fg-tertiary">{hunks} {hunks === 1 ? 'hunk' : 'hunks'}</span>
          </div>
        )}
        {!shut && !file.rows.length && <div className="px-3 py-1 text-fg-tertiary">{stat ? 'No text changes' : 'No changes'}</div>}
        {!shut && <DiffTable rows={visible} />}
      </section>
    })}
    {onShowAll && total > cap && <button type="button" className="sticky left-0 w-full border-t border-border-subtle bg-layer-1 px-3 py-1 text-project-lapis" onClick={onShowAll}>Show all {total} lines</button>}
  </div>
}

/** The patch's files in patch order, then any file `diff_view_read` counted that has no text hunks (a binary file, a
 * pure rename or mode change), so every changed file gets a section. */
export function diffFiles(view: Pick<DiffViewText, 'text' | 'files'>): { files: DiffFile[]; stats: Map<string, FileStat> } {
  const files = parseUnified(view.text).files
  const stats = new Map(view.files.map((s) => [s.path, s] as const))
  const seen = new Set(files.map((f) => f.path))
  for (const s of view.files) if (!seen.has(s.path)) files.push({ path: s.path, rows: [] })
  return { files, stats }
}

/** `diff_view_read(session, spec)`, fetched once: the diff follows the tree only when the user presses Refresh. */
export function useDiffView(session: string, spec: DiffSpec) {
  const api = useApi()
  const supported = useQuerySupport(['diff_view_read'])
  const query = useQuery({
    queryKey: queryKeys.diffView(session, spec),
    queryFn: () => api.diff_view_read(session, spec),
    enabled: supported,
    staleTime: Infinity,
    refetchOnWindowFocus: false,
    refetchOnReconnect: false,
  })
  return { ...query, supported }
}

/** Rows drawn before Show all: an inline card keeps the edit card's cap, a tile draws far more. */
export const INLINE_ROW_CAP = 400
export const TILE_ROW_CAP = 5000

/** A `show_diff` diff (212 §4): every file a collapsible section with its status and `+N −M`, unified view. `className`
 * is the chrome: the tile's full height, or the card's capped strip. */
export function DiffBody({ session, spec, cap = TILE_ROW_CAP, className }: { session: string; spec: DiffSpec; cap?: number; className?: string }) {
  const q = useDiffView(session, spec)
  const [all, setAll] = useState(false)
  const parsed = useMemo(() => (q.data ? diffFiles(q.data) : null), [q.data])
  const box = cn('min-h-0 overflow-auto font-mono text-xs leading-5', className)
  if (!q.supported) return <Unavailable />
  if (q.error) return <p className={cn(box, 'p-3 text-danger wrap-anywhere')}>could not load diff: {String(q.error instanceof Error ? q.error.message : q.error)}</p>
  if (!parsed || !q.data) {
    return <div className={cn(box, 'flex flex-col gap-2 p-3')} aria-busy="true">
      {[70, 45, 85, 60].map((w) => <div key={w} className="h-3 animate-pulse rounded bg-layer-2" style={{ width: `${w}%` }} />)}
    </div>
  }
  if (!parsed.files.length) return <div className={cn(box, 'p-3 text-fg-tertiary')}>No changes</div>
  return <div className={cn(box, 'flex flex-col')}>
    <DiffSections files={parsed.files} stats={parsed.stats} cap={all ? Infinity : cap} onShowAll={() => setAll(true)} />
    {q.data.truncated && <div className="border-t border-border-subtle px-3 py-1 text-fg-tertiary">The diff is truncated; the files above are not all of it.</div>}
  </div>
}
