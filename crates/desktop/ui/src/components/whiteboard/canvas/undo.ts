// §4.6 / §8.4: one Y.UndoManager per view over the item maps, tracking only the owner's origin — engine updates
// ('engine', 'restore') and bookkeeping ('wb-local') never enter it.
//
// Review M1: `Y.UndoManager` replays one stack item as one transaction. If a chunked bulk write (§2.9) were merged
// into one stack item, undoing it would send one update over the provider's 48 KiB cap and close the document. So
// stack items are never merged (`captureTimeout: 0`): every transaction — every chunk — is its own item, and the
// chunks of one bulk write carry a group id (`GROUP` in the transaction's meta, set by `ops.chunked`). Undo and redo
// replay a whole group, one chunk per transaction: one press for the owner, every update under the cap.
import * as Y from 'yjs'
import { OWNER } from './model'

/** Transaction / stack-item meta key: the bulk write a chunk belongs to. */
export const GROUP = 'wb-group'

export type WbUndo = {
  undo(): void
  redo(): void
  canUndo(): boolean
  canRedo(): boolean
  /** Kept for callers that mark step boundaries; items are never merged, so this only resets Yjs' capture state. */
  stopCapturing(): void
  destroy(): void
  readonly manager: Y.UndoManager
}

export function createUndo(doc: Y.Doc): WbUndo {
  const maps = (['cards', 'links', 'frames', 'ink', 'marks'] as const).map((n) => doc.getMap<unknown>(n))
  // Registered before the manager's own handler, so the group of the transaction that is about to become a stack
  // item is known when 'stack-item-added' fires.
  let txGroup: unknown
  const onTx = (tr: Y.Transaction) => {
    txGroup = tr.meta.get(GROUP)
  }
  doc.on('afterTransaction', onTx)
  const manager = new Y.UndoManager(maps, { trackedOrigins: new Set([OWNER]), captureTimeout: 0 })
  // While replaying a group, the inverse items it creates join the same group (so redo replays it whole too).
  let replaying: unknown
  manager.on('stack-item-added', ({ stackItem }) => {
    const g = manager.undoing || manager.redoing ? replaying : txGroup
    if (g !== undefined) stackItem.meta.set(GROUP, g)
  })
  const replay = (stack: Y.UndoManager['undoStack'], step: () => unknown) => {
    const top = stack[stack.length - 1]
    if (!top) return
    const g = top.meta.get(GROUP)
    replaying = g
    try {
      do step()
      while (g !== undefined && stack[stack.length - 1]?.meta.get(GROUP) === g)
    } finally {
      replaying = undefined
    }
  }
  return {
    manager,
    undo: () => replay(manager.undoStack, () => manager.undo()),
    redo: () => replay(manager.redoStack, () => manager.redo()),
    canUndo: () => manager.canUndo(),
    canRedo: () => manager.canRedo(),
    stopCapturing: () => manager.stopCapturing(),
    destroy() {
      doc.off('afterTransaction', onTx)
      manager.destroy()
    },
  }
}
