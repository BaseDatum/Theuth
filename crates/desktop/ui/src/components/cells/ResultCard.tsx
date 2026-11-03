// Owner: U1
import { Eye } from 'lucide-react'
import type { CellRunId } from '@/rpc/bindings/CellRunId'
import { registeredCells, scopeKey, useCells } from './store'

export type ResultCardProps = {
  /** The `<cell_result run=…>` input tag's run. */
  run: CellRunId
  session: string
  // 171:U1-ADD the tag's other attributes (`injected.ts` `cell_result`)
  name?: string
  cell?: string
  exit: string
  secs: string
}

/**
 * 171 §9: the harness input card for a cell result in the transcript (`injected.ts` `CARD.cell_result`): one quiet line
 * "theuth read · Preview · exit 0 · 41 s" — the output is already on screen in the cell. The label comes from the
 * mounted cell that ran it, else the tag's name, else `cell <ordinal>`.
 */
export function ResultCard({ run, session, name, cell, exit, secs }: ResultCardProps) {
  const scope = { type: 'session', id: session } as const
  const ref = useCells((st) => Object.values(st.scopes[scopeKey(scope)]?.states ?? {}).find((s) => s.latest?.id === run)?.cell)
  const label = (ref && registeredCells(scope).find((c) => JSON.stringify(c.ref) === JSON.stringify(ref))?.label) ?? name ?? (cell ? `cell ${cell}` : run)
  const code = /^-?\d+$/.test(exit) ? `exit ${exit}` : exit
  return (
    <p className="my-1 flex items-center gap-1.5 text-xs text-fg-tertiary">
      <Eye className="size-3.5 shrink-0" aria-hidden />
      {['theuth read', label, code, secs && `${secs} s`].filter(Boolean).join(' · ')}
    </p>
  )
}
