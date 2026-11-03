// Owner: U2
import type { ReactNode } from 'react'
import type { CellSpec } from '@/rpc/bindings/CellSpec'
import type { DocId } from '@/rpc/bindings/DocId'
import type { NoteKey } from '@/rpc/bindings/NoteKey'

export type NoteCellProps = {
  doc: DocId
  /** `Name(name)` when the header names the cell, else `Block(id)`. */
  key_: NoteKey
  cell: CellSpec
  code: string
  /** BlockNote's own code content (`contentRef`), §4.2. */
  editor: ReactNode
  editable: boolean
}

/** 171 §4.2: a note code block with a `#|` header, mounted from `editor/CodeBlock.tsx`. */
export function NoteCell(_props: NoteCellProps) {
  return null
}
