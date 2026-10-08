// 59 §7.3: what the note page remembers about its chat panel — open, height, and the last session it
// talked in — per note. `localStorage` + zustand like `meeting-settings.ts`; nothing is pinned server-side.

import { create } from 'zustand'

const KEY = 'note-chat'

export const DEFAULT_HEIGHT = 320
export const MIN_HEIGHT = 160

export type NoteChat = { open: boolean; height: number; session: string | null }

type Saved = Record<string, NoteChat>

type Store = {
  notes: Saved
  set(note: string, patch: Partial<NoteChat>): void
}

function load(): Saved {
  try {
    return JSON.parse(localStorage.getItem(KEY) ?? '{}') as Saved
  } catch {
    return {}
  }
}

/** 143 D9: a note the user never touched opens with the chat under it; a saved `open: false` wins through `set`'s merge. */
export const EMPTY: NoteChat = { open: true, height: DEFAULT_HEIGHT, session: null }

/**
 * 143 D10: the session panel's height for a `saved` height in a pane `pane` px tall. Desktop: the saved height,
 * kept between `MIN_HEIGHT` and `pane - 120` (editor and bar stay visible). Mobile: capped at 45 % of the pane, and
 * a never-dragged panel (`saved === DEFAULT_HEIGHT`) starts at `min(320, 40 %)`. Never below `MIN_HEIGHT` unless the
 * pane itself is smaller than that.
 */
export function panelHeight(saved: number, pane: number, mobile: boolean): number {
  const max = mobile ? 0.45 * pane : pane - 120
  const want = mobile && saved === DEFAULT_HEIGHT ? Math.min(DEFAULT_HEIGHT, 0.4 * pane) : saved
  return Math.max(Math.min(want, max), Math.min(MIN_HEIGHT, pane))
}

export const useNoteChat = create<Store>()((set, get) => ({
  notes: load(),
  set(note, patch) {
    const notes = { ...get().notes, [note]: { ...EMPTY, ...get().notes[note], ...patch } }
    localStorage.setItem(KEY, JSON.stringify(notes))
    set({ notes })
  },
}))
