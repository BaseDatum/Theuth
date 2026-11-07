// Owner: U1
// 171 §11.2: the one state a cell shows, derived from the engine's record, the parsed cell and the local edit.
import type { CellSpec } from '@/rpc/bindings/CellSpec'
import type { Interpreter } from '@/rpc/bindings/Interpreter'
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
  if (disarmedReason(state, cell) !== null) return 'disarmed'
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

/**
 * 171 §7.6 (CELLAFTER): why an `after` cell's gate is not armed (the engine's reason), `null` when it is or there is no
 * `after`. The cell warns with it and offers `RUN_ANYWAY`; a press then sends `confirmed`.
 */
export function disarmedReason(state: CellState | null, cell: CellSpec): string | null {
  const after = cell.options.after
  if (!after || state?.armed) return null
  return state?.disarmed ?? `after: ${after} is not armed`
}

/** The Run button of a disarmed cell. */
export const RUN_ANYWAY = 'Run anyway'

/** The button's text and accessible name (§11.9): the label, else `Run`. */
export const labelOf = (cell: CellSpec) => cell.options.label ?? 'Run'

/** The header's title: the label, else the name, else `cell <ordinal>`. */
export const titleOf = (cell: CellSpec, ordinal?: number) => cell.options.label ?? cell.options.name ?? (ordinal ? `cell ${ordinal}` : 'cell')

/** F4 (171F B21): a chat cell's code takes edits whenever it could be pressed and no run, newer version or error holds it. */
export const chatEditable = (view: CellViewState, runnable: boolean) =>
  runnable && view !== 'running' && view !== 'queued' && view !== 'superseded' && view !== 'invalid' && view !== 'guest'

/** §6.3 (171F B27): the interpreter chip — `auto → python 3.14.8 (system)` when the cell left the pick to `auto`. */
export function interpreterText(cell: CellSpec, interpreter: Interpreter | null): string {
  if (!interpreter) return cell.lang
  const text = [interpreter.kind, interpreter.version, `(${interpreter.source})`].filter(Boolean).join(' ')
  const auto = cell.lang === 'python' && (cell.options.python === null || cell.options.python.kind === 'auto')
  return auto ? `auto → ${text}` : text
}
