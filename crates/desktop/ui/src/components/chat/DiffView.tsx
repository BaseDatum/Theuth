import { useMemo, useState } from 'react'
import { type DiffModel, type DiffPair, type DiffRow, type WordPart, pairsDiff, parseUnified, wordChanges } from '@/lib/editDiff'
import { cn } from '@/lib/utils'

const ROW_CAP = 400
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

export function DiffStats({ added, removed }: Pick<DiffModel, 'added' | 'removed'>) {
  return <span className="inline-flex shrink-0 gap-2 font-mono text-xs" aria-label={`${added} added, ${removed} removed`}>
    <span className="text-success">+{added}</span><span className="text-danger">−{removed}</span>
  </span>
}

/** Unified patches and argument-only snippets share the same file/hunk/line layout. */
export function DiffView({ model, diff, pairs }: { model?: DiffModel; diff?: string; pairs?: DiffPair[] }) {
  const parsed = useMemo(() => model ?? (diff !== undefined ? parseUnified(diff) : pairsDiff(pairs ?? [])), [model, diff, pairs])
  const [all, setAll] = useState(false)
  const total = parsed.files.reduce((n, file) => n + file.rows.length, 0)
  let remaining = all ? Infinity : ROW_CAP
  return <div className="mt-2 -mx-4 max-h-[300px] overflow-auto border-t border-border-subtle font-mono text-xs leading-5" aria-label="File diff">
    {parsed.files.map((file, f) => {
      if (remaining <= 0) return null
      const visible = file.rows.slice(0, remaining)
      remaining -= visible.length
      const marks = marksOf(visible)
      const hunks = file.rows.reduce((n, row) => n + Number(row.kind === 'hunk'), 0)
      return <section key={f} aria-label={file.path || 'Edited file'}>
        <div className="sticky top-0 z-10 flex gap-3 border-b border-border-subtle bg-layer-1 px-3 py-1 text-fg-secondary">
          <span className="break-all">{file.path || 'Edited file'}</span>
          <span className="shrink-0 text-fg-tertiary">{hunks} {hunks === 1 ? 'hunk' : 'hunks'}</span>
        </div>
        {!file.rows.length && <div className="px-3 py-1 text-fg-tertiary">No changes</div>}
        <table className="w-full border-collapse"><tbody>
          {visible.map((row, i) => row.kind !== 'line' ? <tr key={i}>
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
      </section>
    })}
    {!all && total > ROW_CAP && <button type="button" className="sticky left-0 w-full border-t border-border-subtle bg-layer-1 px-3 py-1 text-project-lapis" onClick={() => setAll(true)}>Show all {total} lines</button>}
  </div>
}
