// Composer state that outlives the composer, by session (`''` = new chat): a screen switch unmounts it, the draft
// stays. `drafts` is the typed-but-unsent text; `useSkillDraft` a skill parked for the next send (`/skill:<name>`
// or Use in the Skills panel), shown as a `◇ name` chip and sent as `use_skill(session, name, words)`, so no turn
// starts before the user does. In memory only.
// ponytail: move to localStorage if drafts should survive a relaunch.

import { create } from 'zustand'

export const drafts = new Map<string, string>()

type Store = {
  by: Record<string, string>
  set(session: string, name: string | null): void
}

export const useSkillDraft = create<Store>()((set) => ({
  by: {},
  set: (session, name) =>
    set((s) => {
      const { [session]: _, ...rest } = s.by
      return { by: name === null ? rest : { ...rest, [session]: name } }
    }),
}))
