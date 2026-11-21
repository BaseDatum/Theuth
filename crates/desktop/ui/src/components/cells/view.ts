// Owner: U1
// 171 §11.2: the one state a cell shows, derived from the engine's record, the parsed cell and the local edit.
import type { Cell as CellSpec } from '@/rpc/bindings/auxiliary.surface.output'
import type { Interpreter } from "@/rpc/bindings/method.cells.output"
import type { CellState } from "@/rpc/bindings/method.cells.output"
import { shortLabel } from './grammar'
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
 * 171 §7.6 (CELLAFTER), #1: why an `after` cell's gate is not armed (the engine's reason), `null` when it is or there is
 * no `after`. `after` is advisory: a press asks once (`gateQuestion`) and `RUN_ANYWAY` sends `confirmed`.
 */
export function disarmedReason(state: CellState | null, cell: CellSpec): string | null {
  const after = cell.options.after
  if (!after || state?.armed) return null
  return state?.disarmed ?? `after: ${after} is not armed`
}

/** #1: the one inline question a press of a disarmed cell asks — "preview `x` hasn't run — Run anyway?". */
export function gateQuestion(after: string, reason: string): string {
  const why = reason.replace(/ — run it (?:again|first)$/, '')
  const head = `after: ${after}`
  const rest = why.startsWith(head) ? why.slice(head.length).replace(/ is not armed$/, '') : null
  const what = rest === null ? why : `preview \`${after}\` ${rest === '' ? 'hasn’t run' : rest.trim()}`
  return `${what} — Run anyway?`
}

/** The Run button of a disarmed cell (the inline question's answer). */
export const RUN_ANYWAY = 'Run anyway'

/** The button's text and accessible name (§11.9): the label (#1: cut to `LABEL_MAX` with `…`; the full one is its tooltip), else `Run`. */
export const labelOf = (cell: CellSpec) => (cell.options.label === null ? 'Run' : shortLabel(cell.options.label))

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
