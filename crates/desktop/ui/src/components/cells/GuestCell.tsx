// Owner: U2
import type { CellLatest } from '@/rpc/bindings/CellLatest'
import type { CellSpec } from '@/rpc/bindings/CellSpec'

export type GuestCellProps = {
  cell: CellSpec
  code: string
  /** `cell_latest` row for this cell, or none yet. */
  latest: CellLatest | null
}

/** 171 §13: a member's or guest's view — latest output, `latest run · owner · 14:31`, no button. */
export function GuestCell(_props: GuestCellProps) {
  return null
}
