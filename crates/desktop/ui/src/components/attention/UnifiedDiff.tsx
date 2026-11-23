import { ChevronRight } from 'lucide-react'
import { cn } from '@/lib/utils'
import { parseUnified } from '@/lib/editDiff'

type Row = { op: ' ' | '+' | '-'; text: string; old: number | null; new: number | null }
type Hunk = { header: string; rows: Row[] }
type DiffFile = { path: string; hunks: Hunk[]; added: number; removed: number }

/**
 * `lib/editDiff`'s parser (hunk lengths tell a file header from a removed `-- x`) in the shape this view renders. Files
 * without a hunk are dropped, so `[]` means "not a diff" and the caller shows the raw text. Each `diff --git a/x b/x`
 * section is parsed on its own so a hunk with no `---`/`+++` pair before it is named after its own section. An empty
 * line inside a hunk is a context line (an editor stripped its space); a `[diff truncated: …]` line ends the hunk
 * unrendered.
 */
export function unifiedFiles(text: string): DiffFile[] {
  const sections: { name: string; lines: string[] }[] = [{ name: '', lines: [] }]
  text.split('\n').forEach((line, i, all) => {
    if (line.startsWith('diff --git ')) sections.push({ name: /^diff --git a\/.+? b\/(.+)$/.exec(line)?.[1] ?? '', lines: [] })
    // The `''` after the final newline is not a line.
    sections.at(-1)!.lines.push(line === '' && i < all.length - 1 ? ' ' : line)
  })
  const files: DiffFile[] = []
  for (const section of sections) {
    for (const f of parseUnified(section.lines.join('\n'), section.name).files) {
      const file: DiffFile = { path: f.path, hunks: [], added: 0, removed: 0 }
      let hunk: Hunk | undefined
      for (const r of f.rows) {
        if (r.kind === 'hunk') {
          hunk = { header: r.text, rows: [] }
          file.hunks.push(hunk)
          continue
        }
        if (!hunk) continue
        if (r.kind === 'note') {
          if (r.text.startsWith('[diff truncated:')) hunk = undefined
          // `\ No newline at end of file`: informational, no line numbers.
          else hunk.rows.push({ op: ' ', text: r.text, old: null, new: null })
          continue
        }
        const op = r.op ?? ' '
        hunk.rows.push({ op, text: r.text, old: r.old ?? null, new: r.next ?? null })
        if (op === '+') file.added++
        if (op === '-') file.removed++
      }
      if (file.hunks.length > 0) files.push(file)
    }
  }
  return files
}

/**
 * 101 D10: a `diff` pointer's `text` as files → hunks → +/− rows. Each file is a native `<details>` (open by default) whose
 * summary carries the path and `+N −M`; hunk headers muted; 12 px mono; the body scrolls sideways and never breaks a
 * word. Text that parses to no file falls back to a plain `<pre>`.
 */
export function UnifiedDiff({ text }: { text: string }) {
  const files = unifiedFiles(text)
  if (files.length === 0) {
    return <pre className="overflow-x-auto rounded-md border bg-layer-2 p-3 font-mono text-xs leading-5 break-normal wrap-normal whitespace-pre">{text}</pre>
  }
  return (
    <div className="flex flex-col gap-2">
      {files.map((f, i) => (
        <File key={`${f.path}:${i}`} file={f} />
      ))}
    </div>
  )
}

function File({ file }: { file: DiffFile }) {
  return (
    <details open className="group overflow-hidden rounded-md border text-xs">
      <summary className="flex cursor-pointer list-none items-center gap-2 border-b bg-layer-2 px-3 py-1.5 font-mono [&::-webkit-details-marker]:hidden">
        <ChevronRight className="size-3.5 shrink-0 text-fg-tertiary transition-transform group-open:rotate-90" />
        <span className="min-w-0 flex-1 truncate text-foreground">{file.path || '(unnamed)'}</span>
        <span className="shrink-0 tabular-nums">
          <span className="text-success">+{file.added}</span> <span className="text-danger">−{file.removed}</span>
        </span>
      </summary>
      <div className="overflow-x-auto font-mono leading-5">
        <div className="min-w-max py-0.5">
          {file.hunks.map((h, i) => (
            <div key={i}>
              <div className="px-3 text-fg-tertiary select-none">{h.header}</div>
              {h.rows.map((r, j) => (
                <div key={j} className={cn('grid grid-cols-[42px_42px_14px_1fr] pr-3', r.op === '+' && 'bg-success-bg', r.op === '-' && 'bg-danger-bg')}>
                  <span className="text-right text-fg-tertiary tabular-nums select-none" aria-hidden>
                    {r.old ?? ''}
                  </span>
                  <span className="text-right text-fg-tertiary tabular-nums select-none" aria-hidden>
                    {r.new ?? ''}
                  </span>
                  <span className={cn('text-center select-none', r.op === '+' ? 'text-success' : r.op === '-' ? 'text-danger' : 'text-fg-tertiary')}>{r.op.trim()}</span>
                  <span className="break-normal wrap-normal whitespace-pre">{r.text}</span>
                </div>
              ))}
            </div>
          ))}
        </div>
      </div>
    </details>
  )
}
