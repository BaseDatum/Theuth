// Owner: contract (171 §11, §15). Shared by U1 (chat) and U2 (notes).
import type { ReactNode } from 'react'
import type { CellOutput } from "@/rpc/bindings/method.cells.output"
import type { CellRef } from "@/rpc/bindings/method.cells.output"
import type { CellRun } from "@/rpc/bindings/method.cells.output"
import type { RunMode as CellRunMode } from "@/rpc/bindings/method.cells.output"
import type { Cell as CellSpec } from '@/rpc/bindings/auxiliary.surface.output'
import type { CellState } from "@/rpc/bindings/method.cells.output"

/** 171 §11.2: what the cell looks like; derived from `CellState`, the latest run and the editor. */
export type CellViewState =
  | 'ready'
  | 'queued'
  | 'running'
  | 'done'
  | 'failed'
  | 'stopped'
  | 'lost'
  | 'superseded'
  | 'stale'
  | 'edited'
  | 'invalid'
  | 'armed'
  | 'disarmed'
  | 'guest'

/** The host chip (§11.1): shown only when the run is not on this device. */
export type HostChip = { engine: string; label: string } | null

/** What a cell can do; `CellFence` (chat) and `NoteCell` (notes) bind these to the RPCs of §15.2. */
export type CellCallbacks = {
  /** `confirmed`: the user pressed past a warning (Run anyway, a danger line's confirm); `false` unless passed. */
  onRun(mode: CellRunMode, confirmed?: boolean): void
  onStop(kill: boolean): void
  onInput(data: string): void
  onAsk(): void
  onEdit(code: string): void
  /** Attach to the live run's bytes (`cell_attach`); the returned function detaches. */
  onAttach(): () => void
}

/** Props of the one `Cell` component (§11.1). */
export type CellProps = {
  cellRef: CellRef
  cell: CellSpec
  code: string
  runnable: boolean
  view: CellViewState
  state: CellState | null
  latest: CellRun | null
  host: HostChip
  callbacks: CellCallbacks
  /** chat: CodeMirror; note: BlockNote's code content. */
  editor: ReactNode
  readOnly?: boolean
}

/** Props of the output area (§11.1 output + footer). */
export type OutputViewProps = {
  run: CellRun
  outputs: CellOutput[]
  live: boolean
  tailLines: number
}
