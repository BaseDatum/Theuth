// Owner: U1
import type { CellRun } from '@/rpc/bindings/CellRun'

export type QuitSheetProps = {
  /** `cell_live()`: the runs quitting would end. */
  live: CellRun[]
  onQuit(): void
  onCancel(): void
}

/** 171 §5.3: the pre-quit sheet listing live cell runs (`App.tsx` quit). */
export function QuitSheet(_props: QuitSheetProps) {
  return null
}
