import type { OnResizeEnd } from '@xyflow/react'
import { useCallback, useRef } from 'react'

/**
 * `NodeResizer`'s `onResizeEnd`, with a stable identity. The resizer re-binds its d3 drag whenever the callback changes,
 * and every resize step re-renders the node: a finger's or the Pencil's drag listens on the handle itself, so a fresh
 * callback per render dropped the gesture after its first step (the "limit" a card would not resize past on the iPad).
 * The mouse survived only because d3 listens on `window` for it.
 */
export function useResizeEnd(fn: OnResizeEnd): OnResizeEnd {
  const latest = useRef(fn)
  latest.current = fn
  return useCallback<OnResizeEnd>((e, p) => latest.current(e, p), [])
}
