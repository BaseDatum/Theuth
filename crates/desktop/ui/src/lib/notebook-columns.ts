import { create } from 'zustand'

// 144 §12.1: the side columns collapse to a 12 px rail, remembered per device (like `note-chat.ts`).
const KEY = 'notebook-columns'
export const SIDE_PX = 320
export const RAIL_PX = 12

export type Side = 'sources' | 'notes'
export type Collapsed = Record<Side, boolean>
export const OPEN: Collapsed = { sources: false, notes: false }

function load(): Collapsed {
  try {
    return { ...OPEN, ...(JSON.parse(globalThis.localStorage?.getItem(KEY) ?? '{}') as Partial<Collapsed>) }
  } catch {
    return OPEN
  }
}

export const useNotebookColumns = create<{ collapsed: Collapsed; toggle(side: Side): void }>()((set, get) => ({
  collapsed: load(),
  toggle(side) {
    const collapsed = { ...get().collapsed, [side]: !get().collapsed[side] }
    localStorage.setItem(KEY, JSON.stringify(collapsed))
    set({ collapsed })
  },
}))

export type Tab = 'sources' | 'chat' | 'notes'

/**
 * Pure: how the page lays out. Under 768 px (`useIsMobile`) one column with tabs; otherwise the three-column grid,
 * each collapsed side a 12 px rail (`320px | 1fr | 320px`, mock `:16`).
 */
export function layout(mobile: boolean, collapsed: Collapsed): { mode: 'tabs' } | { mode: 'columns'; template: string } {
  if (mobile) return { mode: 'tabs' }
  const sources = collapsed.sources ? RAIL_PX : SIDE_PX
  const notes = collapsed.notes ? RAIL_PX : SIDE_PX
  return { mode: 'columns', template: `${sources}px minmax(0,1fr) ${notes}px` }
}
