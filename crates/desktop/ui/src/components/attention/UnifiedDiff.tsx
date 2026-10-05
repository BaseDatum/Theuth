import { ChevronRight } from 'lucide-react'
import { cn } from '@/lib/utils'
import { type DiffFile, parseUnifiedDiff } from './diff'

/**
 * 101 D10: a `diff` pointer's `text` as files → hunks → +/− rows. Each file is a native `<details>` (open by default) whose
 * summary carries the path and `+N −M`; hunk headers muted; 12 px mono; the body scrolls sideways and never breaks a
 * word. Text that parses to no file falls back to a plain `<pre>`.
 */
export function UnifiedDiff({ text }: { text: string }) {
  const files = parseUnifiedDiff(text)
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
