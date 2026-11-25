// Shared, stable things the node and edge renderers read (React Flow node data stays small: the card object only).
import { createContext, useContext } from 'react'
import type * as Y from 'yjs'
import type { CanvasHost } from './api'
import type { Lod } from './lod'
import type { Board, Id } from './model'
import type { WbStore } from './store'

export type CanvasCtx = {
  doc: Y.Doc
  store: WbStore
  host: CanvasHost
  board: Board
  lod: Record<Id, Lod>
  dark: boolean
  readOnly: boolean
  /** Live page frames by card (for describe-point captions). */
  frames: Map<Id, HTMLIFrameElement>
  /** Owner actions that need canvas state. */
  enter(id: Id | null): void
  open(id: Id): void
  /** After an owner write that must be one undo step. */
  stopCapturing(): void
  /** Spotlit card (meta.focus / handle.focus with spotlight), null otherwise. */
  spot: Id | null
}

export const Ctx = createContext<CanvasCtx | null>(null)

export function useCanvas(): CanvasCtx {
  const c = useContext(Ctx)
  if (!c) throw new Error('whiteboard canvas context missing')
  return c
}
