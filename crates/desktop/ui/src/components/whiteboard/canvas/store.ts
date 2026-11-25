import { createStore, type StoreApi } from 'zustand/vanilla'
import type { Id, InkColour, InkTool } from './model'

export type Mode = 'idle' | 'draw' | 'erase' | 'link' | 'frame' | 'select'
export type WbView = {
  mode: Mode
  tool: InkTool; colour: InkColour; width: number   // the current pen (defaults: 'pen', 'ink', 2.8)
  fingerInk: boolean        // R2: false by default; ⋯ → "Draw with finger"
  inkEngine: 'auto' | 'pointer'   // ⋯ → Ink engine; 'auto' = native when attached
  nativeInk: boolean        // B4 sets it while tauri-plugin-ink is attached and enabled
  follow: boolean           // [stretch P4]
  showAnswered: boolean
  keepControls: boolean
  clusterOpen: boolean      // B4 owns the timer; B3 reads it for the attribution panel (R3)
  composerOpen: boolean
  selection: Id[]           // cards, links, frames, marks
  entered: Id | null        // the card whose frame receives touches (design §4.4)
  penDown: boolean          // B3 (pointer path) or B4 (native 'state' event) sets it
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
    inkEngine: 'auto',
    nativeInk: false,
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
