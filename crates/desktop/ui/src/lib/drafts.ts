// Composer state that outlives the composer, by session (`''` = new chat; `panel:<session>` = a note page's panel):
// a screen switch, a pane remount or a relaunch leaves the draft where it was. `useDrafts.by` is the typed-but-unsent
// text with the held chips (the composer renders it straight from the store, so no local copy can be lost on a
// remount); `useDrafts.edits` is an in-progress edit of a sent message (`MessageUser`'s bubble), kept apart from the
// composer draft so entering/cancelling an edit never touches it and a virtualised row scrolling out keeps the edit.
// Both persist to `localStorage` like `note-chat.ts`. `useSkillDraft` is a skill parked for the next send
// (`/skill:<name>` or Use in the Skills panel), shown as a `◇ name` chip and sent as `use_skill(session, name, words)`.
//
// A send never clears the draft before the engine acks it (70 SEND-NEVER-LOSES): `sending` marks the draft with the
// client input id the send carries, `acked` drops what was sent, and a failure leaves the marked draft where it is,
// so a reload mid-send or a dropped socket lands the text back in the box with the "Not sent" banner; the retry
// reuses the id, which the engine dedupes.

import { create } from 'zustand'
import type { AttachRef } from '@/lib/attach'
import type { Paste, Pastes } from '@/lib/pastes'

const KEY = 'composer-drafts'

/** `unsent`: the client input id of a send not yet acked (in flight, or failed) — the retry reuses it. */
export type Draft = { text: string; pastes: Pastes; unsent?: string }
/** The bubble's state while `turn` of a session is being edited: its body and the reference chips not yet dropped. */
export type Edit = { turn: number; text: string; refs: AttachRef[] }

export const EMPTY: Draft = { text: '', pastes: new Map() }

/** `Pastes` is a `Map`: stored as its entries. */
type Saved = {
  drafts: Record<string, { text: string; pastes: [number, Paste][]; unsent?: string }>
  edits: Record<string, Edit>
}

type Store = {
  by: Record<string, Draft>
  edits: Record<string, Edit>
  set(key: string, draft: Draft): void
  setText(key: string, text: string): void
  setPastes(key: string, pastes: Pastes | ((held: Pastes) => Pastes)): void
  /** A send of the draft under `key` is in flight as client input `id`; the text stays until `acked`. */
  sending(key: string, id: string): void
  /** The engine took `text`: it leaves the box (anything typed after it during the flight stays). */
  acked(key: string, text: string): void
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
    for (const [k, d] of Object.entries(s.drafts ?? {}))
      by[k] = { text: d.text, pastes: new Map(d.pastes), ...(d.unsent ? { unsent: d.unsent } : {}) }
    return { by, edits: s.edits ?? {} }
  } catch {
    return { by: {}, edits: {} }
  }
}

function save(by: Record<string, Draft>, edits: Record<string, Edit>): void {
  const drafts: Saved['drafts'] = {}
  for (const [k, d] of Object.entries(by))
    if (d.text || d.pastes.size) drafts[k] = { text: d.text, pastes: [...d.pastes], ...(d.unsent ? { unsent: d.unsent } : {}) }
  try {
    localStorage.setItem(KEY, JSON.stringify({ drafts, edits } satisfies Saved))
  } catch {
    // Storage full or disabled: the draft still lives in the store for this run.
  }
}

export const useDrafts = create<Store>()((set, get) => ({
  ...load(),
  set(key, draft) {
    // An emptied box has nothing left to retry.
    const { unsent, ...rest } = draft
    const by = { ...get().by, [key]: unsent && (draft.text || draft.pastes.size) ? draft : rest }
    save(by, get().edits)
    set({ by })
  },
  setText(key, text) {
    get().set(key, { ...(get().by[key] ?? EMPTY), text })
  },
  setPastes(key, pastes) {
    const cur = get().by[key] ?? EMPTY
    get().set(key, { ...cur, pastes: typeof pastes === 'function' ? pastes(cur.pastes) : pastes })
  },
  sending(key, id) {
    get().set(key, { ...(get().by[key] ?? EMPTY), unsent: id })
  },
  acked(key, text) {
    const cur = get().by[key] ?? EMPTY
    get().set(
      key,
      cur.text.startsWith(text)
        ? { text: cur.text.slice(text.length), pastes: EMPTY.pastes }
        : { text: cur.text, pastes: cur.pastes },
    )
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

// Two windows on one chat (`lib/popout.ts`) share this key: a save in one replaces the other's copy, so the
// next save there carries every draft forward — last writer wins, per keystroke, never a stale whole map.
if (typeof window !== 'undefined')
  window.addEventListener('storage', (e) => {
    if (e.key === KEY || e.key === null) useDrafts.setState(load())
  })

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

type PickStore = {
  /** The catalog id picked for the next new chat; `null` = the engine's default model. */
  model: string | null
  /** The effort picked for it; `null` = the engine's default effort (a draft cannot ask for the provider default). */
  effort: string | null
  setModel(model: string | null): void
  setEffort(effort: string | null): void
  reset(): void
}

/**
 * 148 R1: the new-chat draft's model/effort pick. It never touches the engine default: the first send creates
 * the session with it (`create_session`), then `reset` clears it; a new draft resets it too.
 * ponytail: one pick for every draft pane; per-pane picks if split drafts ever need to differ.
 */
export const useDraftPick = create<PickStore>()((set) => ({
  model: null,
  effort: null,
  setModel: (model) => set({ model }),
  setEffort: (effort) => set({ effort }),
  reset: () => set({ model: null, effort: null }),
}))
