// 209 §4.10: the form cards of one engine (31 §7.4, beside approvals/asks/secrets in `Stores`): the pending
// `ElementFormRequested`s (labels and names, never a value) and how the resolved ones ended. Fed by `dispatch`
// (`store/sessions.ts`) from the engine's events and by `lib/element-form.ts` from `element_forms_pending`.

import { type StoreApi, createStore } from 'zustand'
import type { ElementFormRequested, ElementFormResolved } from '@/lib/element-form-wire'
import { scoped } from './engine-scope'

/** How a card ended: the engine's outcome, or `elsewhere` while another window's answer is known taken but its
 * `ElementFormResolved` has not arrived yet. */
export type FormOutcome = ElementFormResolved['outcome'] | 'elsewhere'

/** A resolved card: how it ended and the origin and session it was shown for (from its event, when this window saw it). */
export type Resolved = { outcome: FormOutcome; origin: string | null; session: string | null }

type ElementFormsStore = {
  pending: Record<string, ElementFormRequested>
  outcomes: Record<string, Resolved>
  /** Tool call id → request id: the transcript row a card is drawn on. */
  calls: Record<string, string>
  add(e: ElementFormRequested): void
  /** A later outcome replaces an earlier one (`elsewhere`, then the engine's), keeping what is known of the card. */
  resolve(id: string, outcome: FormOutcome): void
  /** The engine's `element_forms_pending`: afterwards the pending set is exactly that, for `session` when given (its
   * cards only: that session's other calls and outcomes are forgotten too, the transcript's receipts stand in), else
   * for every session but those in `keep` (sessions loaded on their own: owned by another engine). */
  replace(list: ElementFormRequested[], scope: { session: string } | { keep: ReadonlySet<string> }): void
}

export type ElementFormsApi = StoreApi<ElementFormsStore>

/** Per-engine form cards (31 §7.4). */
export const createElementForms = (): ElementFormsApi =>
  createStore<ElementFormsStore>()((set) => ({
    pending: {},
    outcomes: {},
    calls: {},
    add: (e) =>
      set((s) => (s.outcomes[e.id] || s.pending[e.id] ? s : { pending: { ...s.pending, [e.id]: e }, calls: { ...s.calls, [e.call_id]: e.id } })),
    resolve: (id, outcome) =>
      set((s) => {
        const { [id]: form, ...pending } = s.pending
        const was = s.outcomes[id]
        const resolved: Resolved = { outcome, origin: form?.origin ?? was?.origin ?? null, session: form?.session ?? was?.session ?? null }
        return { pending, outcomes: { ...s.outcomes, [id]: resolved } }
      }),
    replace: (list, scope) =>
      set((s) => {
        const kept = (e: ElementFormRequested) => ('session' in scope ? e.session !== scope.session : scope.keep.has(e.session))
        const pending: Record<string, ElementFormRequested> = {}
        for (const e of Object.values(s.pending)) if (kept(e)) pending[e.id] = e
        let calls = s.calls
        let outcomes = s.outcomes
        if ('session' in scope) {
          const listed = new Set(list.map((e) => e.id))
          const ofSession = (id: string) => (s.pending[id]?.session ?? s.outcomes[id]?.session) === scope.session && !listed.has(id)
          calls = Object.fromEntries(Object.entries(s.calls).filter(([, id]) => !ofSession(id)))
          outcomes = Object.fromEntries(Object.entries(s.outcomes).filter(([id]) => !ofSession(id)))
        } else {
          calls = { ...calls }
        }
        for (const e of list) {
          if (outcomes[e.id] || e.type !== 'ElementFormRequested') continue
          // A card already on screen keeps its object: its frame (and what the user typed) stays.
          pending[e.id] = s.pending[e.id] ?? e
          calls[e.call_id] = e.id
        }
        return { pending, calls, outcomes }
      }),
  }))

export const useElementForms = scoped((st) => st.elementForms)
