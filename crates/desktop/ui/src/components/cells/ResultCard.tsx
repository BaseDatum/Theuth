// Owner: U1
import { Eye } from 'lucide-react'
import type { RunId as CellRunId } from "@/rpc/bindings/method.cells.output"

export type ResultCardProps = {
  /** The `<cell_result run=…>` input tag's run. */
  run: CellRunId
  // 171:U1-ADD the tag's other attributes (`injected.ts` `cell_result`)
  name?: string
  label?: string
  cell?: string
  exit: string
  secs: string
}

/**
 * 171 §9: the harness input card for a cell result in the transcript (`injected.ts` `CARD.cell_result`): one quiet line
 * "theuth read · Preview · exit 0 · 41 s" — the output is already on screen in the cell. 171F B20: the label is the
 * tag's own (`label`, else `name`, else `cell <ordinal>`), never looked up from whichever cell ran last.
 */
export function ResultCard({ run, name, label, cell, exit, secs }: ResultCardProps) {
  const title = label ?? name ?? (cell ? `cell ${cell}` : run)
  const code = /^-?\d+$/.test(exit) ? `exit ${exit}` : exit
  return (
    <p className="my-1 flex items-center gap-1.5 text-xs text-fg-tertiary">
      <Eye className="size-3.5 shrink-0" aria-hidden />
      {['theuth read', title, code, secs && `${secs} s`].filter(Boolean).join(' · ')}
    </p>
  )
}
