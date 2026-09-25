// Composer state that outlives the composer, by session (`''` = new chat; `panel:<session>` = a note page's panel):
// a screen switch, a pane remount or a relaunch leaves the draft where it was. `useDrafts.by` is the typed-but-unsent
// text with the held chips (the composer renders it straight from the store, so no local copy can be lost on a
// remount); `useDrafts.edits` is an in-progress edit of a sent message (`MessageUser`'s bubble), kept apart from the
// composer draft so entering/cancelling an edit never touches it and a virtualised row scrolling out keeps the edit.
// Both persist to `localStorage` like `note-chat.ts`. `useSkillDraft` is a skill parked for the next send
// (`/skill:<name>` or Use in the Skills panel), shown as a `◇ name` chip and sent as `use_skill(session, name, words)`.

import { create } from 'zustand'
import type { AttachRef } from '@/lib/attach'
import type { Paste, Pastes } from '@/lib/pastes'

const KEY = 'composer-drafts'

export type Draft = { text: string; pastes: Pastes }
/** The bubble's state while `turn` of a session is being edited: its body and the reference chips not yet dropped. */
export type Edit = { turn: number; text: string; refs: AttachRef[] }

export const EMPTY: Draft = { text: '', pastes: new Map() }

/** `Pastes` is a `Map`: stored as its entries. */
type Saved = { drafts: Record<string, { text: string; pastes: [number, Paste][] }>; edits: Record<string, Edit> }

type Store = {
  by: Record<string, Draft>
  edits: Record<string, Edit>
  set(key: string, draft: Draft): void
  setText(key: string, text: string): void
  setPastes(key: string, pastes: Pastes | ((held: Pastes) => Pastes)): void
  /** Start editing `turn` of `session`, seeded with the message; any other edit in that session is dropped. */
  beginEdit(session: string, edit: Edit): void
  updateEdit(session: string, patch: Partial<Omit<Edit, 'turn'>>): void
  /** Cancel or send: the edit goes, the composer draft was never touched. */
  endEdit(session: string): void
}

export function load(): Pick<Store, 'by' | 'edits'> {
  try {
    const s = JSON.parse(localStorage.getItem(KEY) ?? '{}') as Partial<Saved>
    const by: Record<string, Draft> = {}
    for (const [k, d] of Object.entries(s.drafts ?? {})) by[k] = { text: d.text, pastes: new Map(d.pastes) }
    return { by, edits: s.edits ?? {} }
  } catch {
    return { by: {}, edits: {} }
  }
}

function save(by: Record<string, Draft>, edits: Record<string, Edit>): void {
  const drafts: Saved['drafts'] = {}
  for (const [k, d] of Object.entries(by)) if (d.text || d.pastes.size) drafts[k] = { text: d.text, pastes: [...d.pastes] }
  try {
    localStorage.setItem(KEY, JSON.stringify({ drafts, edits } satisfies Saved))
  } catch {
    // Storage full or disabled: the draft still lives in the store for this run.
  }
}

export const useDrafts = create<Store>()((set, get) => ({
  ...load(),
  set(key, draft) {
    const by = { ...get().by, [key]: draft }
    save(by, get().edits)
    set({ by })
  },
  setText(key, text) {
    get().set(key, { text, pastes: get().by[key]?.pastes ?? EMPTY.pastes })
  },
  setPastes(key, pastes) {
    const cur = get().by[key] ?? EMPTY
    get().set(key, { text: cur.text, pastes: typeof pastes === 'function' ? pastes(cur.pastes) : pastes })
  },
  beginEdit(session, edit) {
    const edits = { ...get().edits, [session]: edit }
    save(get().by, edits)
    set({ edits })
  },
  updateEdit(session, patch) {
    const cur = get().edits[session]
    if (!cur) return
    get().beginEdit(session, { ...cur, ...patch })
  },
  endEdit(session) {
    const { [session]: _, ...edits } = get().edits
    save(get().by, edits)
    set({ edits })
  },
}))

type SkillStore = {
  by: Record<string, string>
  set(session: string, name: string | null): void
}

export const useSkillDraft = create<SkillStore>()((set) => ({
  by: {},
  set: (session, name) =>
    set((s) => {
      const { [session]: _, ...rest } = s.by
      return { by: name === null ? rest : { ...rest, [session]: name } }
    }),
}))
