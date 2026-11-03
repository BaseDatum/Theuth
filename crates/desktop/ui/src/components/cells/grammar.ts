// Owner: U1
import type { CellSpec } from '@/rpc/bindings/CellSpec'

/**
 * 171 §3: parse a fence body as a cell. `null` when `lang` is not a cell language or the body has no `#|` header.
 * U1 mirrors `surface::cell` (`crates/surface/src/cell.rs`) against the shared fixtures `crates/surface/tests/cells.json`.
 */
export function cellOf(_lang: string, _text: string): CellSpec | null {
  return null
}
