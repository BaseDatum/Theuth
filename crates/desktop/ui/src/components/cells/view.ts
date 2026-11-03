// Owner: U1
// 171 §11.2: the one state a cell shows, derived from the engine's record, the parsed cell and the local edit.
import type { CellSpec } from '@/rpc/bindings/CellSpec'
import type { CellState } from '@/rpc/bindings/CellState'
import type { CellViewState } from './types'

export function viewOf(state: CellState | null, cell: CellSpec, edited: boolean, guest = false): CellViewState {
  if (guest) return 'guest'
  if (state?.problems.some((p) => p.severity === 'error')) return 'invalid'
  if (state?.superseded_by) return 'superseded'
  const run = state?.latest
  if (run?.state === 'queued') return 'queued'
  if (run?.state === 'running') return 'running'
  if (run?.state === 'lost') return 'lost'
  if (edited) return 'edited'
  if (cell.options.after && !state?.armed) return 'disarmed'
  if (state?.stale) return 'stale'
  switch (run?.state) {
    case 'done':
      return 'done'
    case 'failed':
      return 'failed'
    case 'stopped':
    case 'killed':
    case 'timeout':
      return 'stopped'
  }
  return state?.armed ? 'armed' : 'ready'
}

/** The button's text and accessible name (§11.9): the label, else `Run`. */
export const labelOf = (cell: CellSpec) => cell.options.label ?? 'Run'

/** The header's title: the label, else the name, else `cell <ordinal>`. */
export const titleOf = (cell: CellSpec, ordinal?: number) => cell.options.label ?? cell.options.name ?? (ordinal ? `cell ${ordinal}` : 'cell')
