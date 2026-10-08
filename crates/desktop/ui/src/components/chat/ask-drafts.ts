// 138 ASK-OWN: what the owner has drafted on an `ask_user`, keyed by ask id and shared by the chat card (`AskCard`)
// and the Attention Ask screen (`AskScreen`). Per question two independent things: the PICK (an option, the typed
// text, a skip, nothing) and the OWN text sitting in the `Type your own…` box. Picking an option or Skip changes the
// pick only — the words stay in the box; they are sent only while they are the pick. In memory only: a relaunch
// starts the ask fresh.
import { useCallback, useEffect, useSyncExternalStore } from 'react'
import { createStore } from 'zustand'
import type { Answer } from '@/rpc/bindings/Answer'
import type { DoublePredicted } from '@/rpc/bindings/DoublePredicted'
import type { Question } from '@/rpc/bindings/Question'
import { preselect } from './DoubleChip'

/** Draft answer per question: an option, typed text, an explicit skip, or nothing yet. */
export type Draft = Answer | null

/** The starting drafts: the recommended option pre-picked, else nothing (the attention Ask screen starts the same way, 104). */
export const draftsFor = (questions: readonly Question[]): Draft[] => questions.map((q) => (q.recommended === undefined ? null : { kind: 'choice', index: q.recommended }))

/** Every question has an option, non-blank text, or an explicit skip. */
export const allAnswered = (drafts: readonly Draft[]): boolean => drafts.every((d) => d !== null && !(d.kind === 'text' && d.text.trim() === ''))

/**
 * 126 §9.2: the double's L1 pre-selection on a one-question ask — `drafts` with question 0 set to the double's option
 * when the owner has not touched it (it still holds the model's starting draft); the same array otherwise. The model's
 * `recommended` stays as rendered, so a disagreement shows both.
 */
export function withDoublePick(drafts: readonly Draft[], questions: readonly Question[], p: DoublePredicted | undefined): Draft[] {
  const q = questions.length === 1 ? questions[0] : undefined
  if (!q) return drafts as Draft[]
  const i = preselect(p, q.options.map((o) => o.label))
  if (i === null) return drafts as Draft[]
  const start = draftsFor(questions)[0] ?? null
  const cur = drafts[0] ?? null
  const untouched = cur === null ? start === null : start !== null && cur.kind === 'choice' && start.kind === 'choice' && cur.index === start.index
  if (!untouched || (cur?.kind === 'choice' && cur.index === i)) return drafts as Draft[]
  return [{ kind: 'choice', index: i }, ...drafts.slice(1)]
}

/** The model's `recommended` label of a one-question ask, for the chip's disagreement badge. */
export function recommendedLabel(questions: readonly Question[]): string | undefined {
  const q = questions.length === 1 ? questions[0] : undefined
  return q?.recommended === undefined ? undefined : q.options[q.recommended]?.label
}

/** What Send puts on the wire for one pick: the option, the trimmed text, else skipped. The own text of a question whose pick is not text never gets here. */
export function toAnswer(d: Draft): Answer {
  if (!d) return { kind: 'skipped' }
  if (d.kind === 'text' && d.text.trim() === '') return { kind: 'skipped' }
  if (d.kind === 'text') return { kind: 'text', text: d.text.trim() }
  return d
}

/** One ask's state: the pick and the box text per question, same length as the questions. */
export type AskDraft = { picks: Draft[]; own: string[] }

export const seedDraft = (questions: readonly Question[]): AskDraft => ({ picks: draftsFor(questions), own: questions.map(() => '') })

const at = <T>(xs: readonly T[], i: number, v: T): T[] => xs.map((x, j) => (j === i ? v : x))

/** Typing (or dictating) in the box: the words are the own text and, at once, the pick. Emptying it leaves a blank text pick, so `allAnswered` is false. */
export const typed = (d: AskDraft, i: number, text: string): AskDraft => ({ picks: at(d.picks, i, { kind: 'text', text }), own: at(d.own, i, text) })

/** An option, Skip, or un-skip (`null`): the pick changes, the words stay. */
export const picked = (d: AskDraft, i: number, pick: Draft): AskDraft => ({ picks: at(d.picks, i, pick), own: d.own })

/** Focusing the box or pressing its ○: non-blank words that are not the pick become the pick; otherwise the pick as is (same object). */
export const ownPick = (pick: Draft, own: string): Draft => (own.trim() === '' || pick?.kind === 'text' ? pick : { kind: 'text', text: own })

type AskDrafts = {
  asks: Record<string, AskDraft>
  /** `f` over the ask's entry (seeded from the questions when absent); an unchanged result writes nothing. */
  update(id: string, questions: readonly Question[], f: (d: AskDraft) => AskDraft): void
  /** The ask is answered or dismissed: forget it. */
  drop(id: string): void
}

export const askDrafts = createStore<AskDrafts>()((set) => ({
  asks: {},
  update: (id, questions, f) =>
    set((s) => {
      const cur = s.asks[id] ?? seedDraft(questions)
      const next = f(cur)
      return next === cur ? s : { asks: { ...s.asks, [id]: next } }
    }),
  drop: (id) =>
    set((s) => {
      if (!(id in s.asks)) return s
      const { [id]: _, ...asks } = s.asks
      return { asks }
    }),
}))

const EMPTY: AskDraft = { picks: [], own: [] }

/**
 * One ask's draft on either surface, plus the actions. Reads the store entry, or the seeded start before the first
 * edit; the double's L1 pre-pick applies through the store while `live` (not after Send) and only to an untouched pick.
 */
export function useAskDraft(ask: { id: string; questions: Question[] } | null, prediction: DoublePredicted | undefined, live = true) {
  const asks = useSyncExternalStore(
    askDrafts.subscribe,
    () => askDrafts.getState().asks,
    () => askDrafts.getState().asks,
  )
  const id = ask?.id
  const questions = ask?.questions
  useEffect(() => {
    if (id && questions && live)
      askDrafts.getState().update(id, questions, (d) => {
        const picks = withDoublePick(d.picks, questions, prediction)
        return picks === d.picks ? d : { ...d, picks }
      })
  }, [id, questions, prediction, live])
  const update = useCallback(
    (f: (d: AskDraft) => AskDraft) => {
      if (id && questions) askDrafts.getState().update(id, questions, f)
    },
    [id, questions],
  )
  const draft = id ? (asks[id] ?? (questions ? seedDraft(questions) : EMPTY)) : EMPTY
  return {
    draft,
    pick: useCallback((i: number, p: Draft) => update((d) => picked(d, i, p)), [update]),
    typeOwn: useCallback((i: number, text: string) => update((d) => typed(d, i, text)), [update]),
    drop: useCallback(() => id && askDrafts.getState().drop(id), [id]),
  }
}
