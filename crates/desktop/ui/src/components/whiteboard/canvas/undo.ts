// §4.6 / §8.4: one Y.UndoManager per view over the item maps, tracking only the owner's origin — engine updates
// ('engine', 'restore') and bookkeeping ('wb-local') never enter it.
import * as Y from 'yjs'
import { OWNER } from './model'

/** Chunked bulk writes land within the capture window and undo as one step; callers `stopCapturing()` after every stroke and drag end. */
export function createUndo(doc: Y.Doc): Y.UndoManager {
  const maps = (['cards', 'links', 'frames', 'ink', 'marks'] as const).map((n) => doc.getMap<unknown>(n))
  return new Y.UndoManager(maps, { trackedOrigins: new Set([OWNER]) })
}
