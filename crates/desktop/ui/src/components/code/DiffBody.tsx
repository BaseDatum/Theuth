// 212 §4: the diff renderer shared by an edit card's `DiffView` (capped, inside the card) and the `show_diff` tile,
// card and popped window (`DiffBody`, every file, collapsible, fetched with `diff_view_read`), unified or split.
import { useQuery } from '@tanstack/react-query'
import { ChevronRight } from 'lucide-react'
import { useCallback, useMemo, useRef, useState } from 'react'
import { ToggleGroup, ToggleGroupItem } from '@/components/ui/toggle-group'
import { type DiffLayout, diffLayout, useDiffLayoutChoice } from '@/lib/diff-layout'
import { type DiffFile, type DiffRow, type WordPart, marksOf, parseUnified, splitRows } from '@/lib/editDiff'
import { queryKeys, useApi } from '@/lib/rpc'
import { cn } from '@/lib/utils'
import type { DiffSpec } from '@/rpc/bindings/method.artifact.input'
import type { DiffFileStat, DiffViewText } from '@/rpc/bindings/method.artifact.output'
import { Unavailable, useQuerySupport } from '@/rpc/capabilities'

const ROW = { ' ': 'text-fg-secondary', '-': 'bg-danger-bg', '+': 'bg-success-bg' }
const NUM = 'select-none px-1.5 text-right align-top text-fg-tertiary'

/** `row`'s text, its changed words (`marksOf`) marked. */
function Words({ row, parts }: { row: DiffRow; parts: WordPart[] | undefined }) {
  return <>{parts?.map((part, n) => !part.changed ? part.text : row.op === '+'
    ? <ins key={n} className="rounded-sm bg-success/25 no-underline">{part.text}</ins>
    : <del key={n} className="rounded-sm bg-danger/25 no-underline">{part.text}</del>) ?? row.text}</>
}

/** One file's hunk headers, notes and numbered lines, changed words marked. */
function DiffTable({ rows }: { rows: DiffRow[] }) {
  const marks = marksOf(rows)
  return <table className="w-full border-collapse"><tbody>
    {rows.map((row, i) => row.kind !== 'line' ? <tr key={i}>
      <td colSpan={4} className={cn('px-3 whitespace-pre text-fg-tertiary', row.kind === 'hunk' && 'bg-project-slate/10')}>{row.text}</td>
    </tr> : <tr key={i} className={ROW[row.op ?? ' ']}>
      <td className={cn('w-px min-w-9', NUM)}>{row.old}</td>
      <td className={cn('w-px min-w-9', NUM)}>{row.next}</td>
      <td className={cn('w-px select-none px-1 align-top', row.op === '+' ? 'text-success' : row.op === '-' ? 'text-danger' : '')}>{row.op === '-' ? '−' : row.op}</td>
      <td className="w-full pr-3 whitespace-pre"><Words row={row} parts={marks.get(i)} /></td>
    </tr>)}
  </tbody></table>
}

/** One side's cells of a split row: its line number and text, or filler where the other side has no counterpart. */
function Side({ rows, at, side, marks }: { rows: DiffRow[]; at: number | null; side: 'old' | 'new'; marks: Map<number, WordPart[]> }) {
  const edge = side === 'old' && 'border-r border-border-subtle'
  const row = at === null ? undefined : rows[at]
  if (at === null || !row) return <td colSpan={2} className={cn('bg-layer-2/60', edge)} data-filler="" />
  const tint = ROW[row.op ?? ' ']
  return <>
    <td className={cn(NUM, tint)}>{side === 'old' ? row.old : row.next}</td>
    <td className={cn('pr-3 pl-1 align-top whitespace-pre-wrap wrap-anywhere', tint, edge)}><Words row={row} parts={marks.get(at)} /></td>
  </>
}

/** One file side by side (`splitRows`): old on the left, new on the right, each side wrapping its long lines so a row
 * stays one aligned row and neither side scrolls out of view. */
function SplitTable({ rows }: { rows: DiffRow[] }) {
  const marks = marksOf(rows)
  return <table className="w-full table-fixed border-collapse">
    <colgroup><col className="w-12" /><col /><col className="w-12" /><col /></colgroup>
    <tbody>
      {splitRows(rows).map((s, n) => s.kind !== 'line' ? <tr key={n}>
        <td colSpan={4} className={cn('px-3 whitespace-pre-wrap wrap-anywhere text-fg-tertiary', s.kind === 'hunk' && 'bg-project-slate/10')}>{rows[s.row]!.text}</td>
      </tr> : <tr key={n}>
        <Side rows={rows} at={s.left} side="old" marks={marks} />
        <Side rows={rows} at={s.right} side="new" marks={marks} />
      </tr>)}
    </tbody>
  </table>
}

export type FileStat = Pick<DiffFileStat, 'status' | 'added' | 'removed'>

/**
 * Files of a parsed patch, each a section under a sticky header, drawing at most `cap` rows across all files (then
 * Show all, when `onShowAll` is given). The caller's `className` is the chrome (an edit card's capped strip, a tile's
 * full height). With `stats` (the `diff_view_read` per-file counts, by path) a header is a fold button carrying the
 * file's status and `+N −M` instead of the hunk count. `layout` `split` draws each file side by side.
 */
export function DiffSections({ files, cap = Infinity, onShowAll, className, stats, layout = 'unified' }: {
  files: DiffFile[]
  cap?: number
  onShowAll?: () => void
  className?: string
  stats?: Map<string, FileStat>
  layout?: DiffLayout
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
        {!shut && (layout === 'split' ? <SplitTable rows={visible} /> : <DiffTable rows={visible} />)}
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

/** The tile and popped window's meta line: `N files +A −R`, and whether the diff is cut short. */
export const diffMeta = (data: Pick<DiffViewText, 'files' | 'added' | 'removed' | 'truncated'>): string =>
  `${data.files.length} ${data.files.length === 1 ? 'file' : 'files'} +${data.added} −${data.removed}${data.truncated ? ' · truncated' : ''}`

/** What a diff `spec` shows, for a window that has only the spec (a popped diff): `staged`, `commit abc1234`, … */
export function diffTitle(spec: DiffSpec): string {
  const short = (sha: string | null | undefined) => (sha ?? '').slice(0, 7)
  const base = (p: string | null | undefined) => (p ?? '').split(/[\\/]/).pop() ?? ''
  const what = spec.mode === 'staged' ? 'staged'
    : spec.mode === 'commit' ? `commit ${short(spec.a)}`
    : spec.mode === 'range' ? `${short(spec.a)}${spec.three_dot ? '...' : '..'}${short(spec.b)}`
    : spec.mode === 'files' ? `${base(spec.a)} ↔ ${base(spec.b)}`
    : 'working tree'
  return spec.paths?.length ? `${what} · ${spec.paths.join(' ')}` : what
}

/** The diff layout a box draws in (`lib/diff-layout.ts`): the remembered choice, else by the width of the element
 * `ref` is put on; `choose` remembers an explicit choice for every diff. */
export function useDiffLayout(): { layout: DiffLayout; choose(layout: DiffLayout): void; ref(el: HTMLElement | null): void } {
  const { chosen, choose } = useDiffLayoutChoice()
  const [width, setWidth] = useState(0)
  const watch = useRef<ResizeObserver | null>(null)
  const ref = useCallback((el: HTMLElement | null) => {
    watch.current?.disconnect()
    watch.current = null
    if (!el) return
    setWidth(el.getBoundingClientRect().width)
    watch.current = new ResizeObserver(([e]) => e && setWidth(e.contentRect.width))
    watch.current.observe(el)
  }, [])
  return { layout: diffLayout(chosen, width), choose, ref }
}

/** The Unified | Split switch of a diff tile, card or popped window. */
export function DiffLayoutToggle({ layout, onChange, className }: { layout: DiffLayout; onChange(layout: DiffLayout): void; className?: string }) {
  return (
    <ToggleGroup type="single" variant="outline" size="sm" value={layout} aria-label="Diff layout" className={className} onValueChange={(v) => (v === 'unified' || v === 'split') && onChange(v)}>
      <ToggleGroupItem value="unified" className="px-2 text-xs">Unified</ToggleGroupItem>
      <ToggleGroupItem value="split" className="px-2 text-xs">Split</ToggleGroupItem>
    </ToggleGroup>
  )
}

/** Rows drawn before Show all: an inline card keeps the edit card's cap, a tile draws far more. */
export const INLINE_ROW_CAP = 400
export const TILE_ROW_CAP = 5000

/** A `show_diff` diff (212 §4): every file a collapsible section with its status and `+N −M`, in `layout`.
 * `className` is the chrome: the tile's full height, or the card's capped strip. */
export function DiffBody({ session, spec, cap = TILE_ROW_CAP, layout = 'unified', className }: { session: string; spec: DiffSpec; cap?: number; layout?: DiffLayout; className?: string }) {
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
    <DiffSections files={parsed.files} stats={parsed.stats} layout={layout} cap={all ? Infinity : cap} onShowAll={() => setAll(true)} />
    {q.data.truncated && <div className="border-t border-border-subtle px-3 py-1 text-fg-tertiary">The diff is truncated; the files above are not all of it.</div>}
  </div>
}
