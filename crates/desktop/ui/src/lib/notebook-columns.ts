import { create } from 'zustand'

// 144 §12.1: the side columns collapse to a 12 px rail, remembered per device (like `note-chat.ts`).
// 157 D7: the notes pane's width is remembered with them (one key, one object).
const KEY = 'notebook-columns'
export const SIDE_PX = 320
export const RAIL_PX = 12
/** 157 D7 clamps: the notes pane never under this, the chat column never under `CHAT_MIN_PX`, the pane at most 60 % of the page. */
export const NOTES_MIN_PX = 280
export const CHAT_MIN_PX = 360
export const NOTES_MAX_RATIO = 0.6

export type Side = 'sources' | 'notes'
export type Collapsed = Record<Side, boolean>
export const OPEN: Collapsed = { sources: false, notes: false }
type Saved = Collapsed & { notesWidth: number }
const DEFAULT: Saved = { ...OPEN, notesWidth: SIDE_PX }

function load(): Saved {
  try {
    const s = JSON.parse(globalThis.localStorage?.getItem(KEY) ?? '{}') as Partial<Saved>
    const w = Number(s.notesWidth)
    return { ...DEFAULT, ...s, notesWidth: Number.isFinite(w) && w >= NOTES_MIN_PX ? w : SIDE_PX }
  } catch {
    return DEFAULT
  }
}

export const useNotebookColumns = create<{ collapsed: Collapsed; notesWidth: number; toggle(side: Side): void; setNotesWidth(px: number): void }>()((set, get) => {
  const { notesWidth, ...collapsed } = load()
  return {
    collapsed,
    notesWidth,
    toggle(side) {
      const collapsed = { ...get().collapsed, [side]: !get().collapsed[side] }
      localStorage.setItem(KEY, JSON.stringify({ ...collapsed, notesWidth: get().notesWidth } satisfies Saved))
      set({ collapsed })
    },
    setNotesWidth(px) {
      const notesWidth = Math.round(px)
      if (notesWidth === get().notesWidth) return
      localStorage.setItem(KEY, JSON.stringify({ ...get().collapsed, notesWidth } satisfies Saved))
      set({ notesWidth })
    },
  }
})

/**
 * Pure (157 D7): the notes pane width a drag or nudge asks for, held between `NOTES_MIN_PX` and whichever is
 * smaller of 60 % of `pageWidth` and what leaves the chat column `CHAT_MIN_PX` beside the sources column
 * (`sourcesWidth`: 320 open, 12 collapsed). Too narrow a page for both: the minimum wins.
 */
export function clampNotesWidth(px: number, pageWidth: number, sourcesWidth: number): number {
  const max = Math.min(Math.floor(pageWidth * NOTES_MAX_RATIO), pageWidth - sourcesWidth - CHAT_MIN_PX)
  return Math.round(Math.max(NOTES_MIN_PX, Math.min(px, max)))
}

export type Tab = 'sources' | 'chat' | 'notes'

/**
 * Pure: how the page lays out. Under 768 px (`useIsMobile`) one column with tabs; otherwise the three-column grid,
 * each collapsed side a 12 px rail (`320px | 1fr | <notes>px`, mock `:16`; the notes width is the remembered one).
 */
export function layout(mobile: boolean, collapsed: Collapsed, notesWidth = SIDE_PX): { mode: 'tabs' } | { mode: 'columns'; template: string } {
  if (mobile) return { mode: 'tabs' }
  const sources = collapsed.sources ? RAIL_PX : SIDE_PX
  const notes = collapsed.notes ? RAIL_PX : notesWidth
  return { mode: 'columns', template: `${sources}px minmax(0,1fr) ${notes}px` }
}
