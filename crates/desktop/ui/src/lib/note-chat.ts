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

export const EMPTY: NoteChat = { open: false, height: DEFAULT_HEIGHT, session: null }

export const useNoteChat = create<Store>()((set, get) => ({
  notes: load(),
  set(note, patch) {
    const notes = { ...get().notes, [note]: { ...EMPTY, ...get().notes[note], ...patch } }
    localStorage.setItem(KEY, JSON.stringify(notes))
    set({ notes })
  },
}))
