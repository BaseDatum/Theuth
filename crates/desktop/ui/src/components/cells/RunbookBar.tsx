// Owner: U1
import type { CellRef } from '@/rpc/bindings/CellRef'
import type { CellState } from '@/rpc/bindings/CellState'
import type { KernelState } from '@/rpc/bindings/KernelState'

export type RunbookBarProps = {
  /** The reply's cells, in order, with their state. */
  cells: { ref: CellRef; state: CellState | null }[]
  kernel: KernelState | null
  onRunAll(): void
  onOpenAsNote(): void
}

/** 171 §11.3: Run all, progress segments and the kernel state above a reply with two or more cells. */
export function RunbookBar(_props: RunbookBarProps) {
  return null
}
