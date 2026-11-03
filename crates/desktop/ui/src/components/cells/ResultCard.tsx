// Owner: U1
import type { CellRunId } from '@/rpc/bindings/CellRunId'

export type ResultCardProps = {
  /** The `<cell_result run=…>` input tag's run. */
  run: CellRunId
  session: string
}

/** 171 §9: the harness input card for a cell result in the transcript (`injected.ts` `CARD.cell_result`). */
export function ResultCard(_props: ResultCardProps) {
  return null
}
