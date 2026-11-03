// Owner: U1
// Lead pin L11: wire fields Z adds to the Rust types at integration. Typed here as OPTIONAL extensions of the
// generated bindings; absent fields render nothing. Once the bindings carry them, these become plain aliases.
import type { CellRun } from '@/rpc/bindings/CellRun'
import type { CellState } from '@/rpc/bindings/CellState'
import type { KernelState } from '@/rpc/bindings/KernelState'

export type CellRunWire = CellRun & {
  /** Values masked in theuth's copy (§9.4); the footer says "N masked for theuth" above 0. */
  masked?: number
  /** A python run waiting on `input()` / `getpass()` (§7.4); answered through `cell_input`, never stored. */
  input?: { prompt: string; password: boolean } | null
  /** The run's cell was `#| private` (§9.1). */
  private?: boolean
  /** The cell's `name`, for lists that have no mounted cell (quit sheet). */
  name?: string | null
}
export type CellStateWire = CellState & {
  /** §14.2: the engine that runs the cell when it is not this device ("Sky's Mac mini"). */
  host?: string | null
}
export type KernelStateWire = KernelState & {
  /** §5.4: the login environment could not be read — the strip's chip. */
  env_warning?: string | null
}
