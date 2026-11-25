import { createStore, type StoreApi } from 'zustand/vanilla'
import type { Id, InkColour, InkTool } from './model'

/** `hand`: no ink from any pointer — the Pencil navigates like a finger (picked from the tools menu, H, or Esc). */
export type Mode = 'idle' | 'hand' | 'draw' | 'erase' | 'link' | 'frame' | 'select'
export type WbView = {
  mode: Mode
  tool: InkTool; colour: InkColour; width: number   // the current pen (defaults: 'pen', 'ink', 2.8)
  fingerInk: boolean        // R2: false by default; ⋯ → "Draw with finger"
  follow: boolean           // [stretch P4]
  showAnswered: boolean
  keepControls: boolean
  clusterOpen: boolean      // B4 owns the timer; B3 reads it for the attribution panel (R3)
  composerOpen: boolean
  selection: Id[]           // cards, links, frames, marks
  entered: Id | null        // the card whose frame receives touches (design §4.4)
  penDown: boolean          // the pointer path sets it while a pen is down
  scrub: Record<Id, number> // card → version shown (view state; absent = current)
}
export type WbStore = StoreApi<WbView & { set(patch: Partial<WbView>): void }>

export function createWhiteboardStore(init?: Partial<WbView>): WbStore {
  return createStore<WbView & { set(patch: Partial<WbView>): void }>()((set) => ({
    mode: 'idle',
    tool: 'pen',
    colour: 'ink',
    width: 2.8,
    fingerInk: false,
    follow: false,
    showAnswered: false,
    keepControls: false,
    clusterOpen: false,
    composerOpen: false,
    selection: [],
    entered: null,
    penDown: false,
    scrub: {},
    ...init,
    set: (patch) => set(patch),
  }))
}
