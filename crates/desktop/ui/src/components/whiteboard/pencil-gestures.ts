// 218: what a Pencil double-tap or squeeze does (the plugin forwards the gesture and the owner's preferred action from
// Settings → Apple Pencil; `lib/ink-native.ts`). The store stays the only writer of the tool: this returns a patch.
import type { PencilEvent } from '@/lib/ink-native'
import type { InkTool } from './canvas/model'
import type { WbStore, WbView } from './canvas/store'

/** The tool before the last change, for "switch to previous". */
export type PreviousTool = { tool: InkTool; erase: boolean }

/**
 * - `switchEraser`: eraser on, or back to the latched pen.
 * - `switchPrevious`: swap with the previous tool (twice returns to the start).
 * - the three palette actions: raise the cluster (its tool picker).
 * - `runSystemShortcut`, `ignore`: nothing.
 */
export function pencilAction(e: PencilEvent, s: Pick<WbView, 'mode' | 'tool'>, previous: PreviousTool): { patch: Partial<WbView> | null; showTools: boolean } {
  switch (e.action) {
    case 'switchEraser':
      return { patch: { mode: s.mode === 'erase' ? 'draw' : 'erase' }, showTools: false }
    case 'switchPrevious':
      return { patch: { tool: previous.tool, mode: previous.erase ? 'erase' : 'draw' }, showTools: false }
    case 'showColorPalette':
    case 'showInkAttributes':
    case 'showContextualPalette':
      return { patch: null, showTools: true }
    default:
      return { patch: null, showTools: false }
  }
}

/** Track the previous tool from the store and apply Pencil gestures to it. Returns the handler and an unsubscribe. */
export function pencilGestures(store: WbStore, showTools: () => void): { on(e: PencilEvent): void; dispose(): void } {
  const st = store.getState()
  let previous: PreviousTool = { tool: st.tool, erase: st.mode === 'erase' }
  const dispose = store.subscribe((s, p) => {
    const was = p.mode === 'erase'
    if (s.tool !== p.tool || (s.mode === 'erase') !== was) previous = { tool: p.tool, erase: was }
  })
  return {
    on(e) {
      const r = pencilAction(e, store.getState(), previous)
      if (r.patch) store.getState().set(r.patch)
      if (r.showTools) showTools()
    },
    dispose,
  }
}
