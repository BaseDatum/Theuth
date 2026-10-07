// 126 §10: what the owner's double said about each pending card, keyed by `question_id` — the `DoublePredicted` /
// `DoubleAnswered` events of the live engine (`on`, `store/sessions`). The engine re-publishes a prediction for every
// pending card on (re)subscribe (§4.4), so a reload is whole again after the first frame. Nothing is persisted.
import { useSyncExternalStore } from 'react'
import { createStore } from 'zustand'
import type { DoubleAnswered } from '@/rpc/bindings/DoubleAnswered'
import type { DoublePredicted } from '@/rpc/bindings/DoublePredicted'
import { on } from './sessions'

type DoubleStore = {
  predicted: Record<string, DoublePredicted>
  answered: Record<string, DoubleAnswered>
  /** The owner took an L2 card over (or undid): the countdown stops, the chip stays. */
  takenOver: Record<string, true>
  predict(e: DoublePredicted): void
  answer(e: DoubleAnswered): void
  takeOver(id: string): void
}

export const doubleStore = createStore<DoubleStore>()((set) => ({
  predicted: {},
  answered: {},
  takenOver: {},
  predict: (e) => set((s) => ({ predicted: { ...s.predicted, [e.question_id]: e } })),
  answer: (e) => set((s) => ({ answered: { ...s.answered, [e.question_id]: e } })),
  takeOver: (id) => set((s) => ({ takenOver: { ...s.takenOver, [id]: true } })),
}))

on('DoublePredicted', (e) => doubleStore.getState().predict(e))
on('DoubleAnswered', (e) => doubleStore.getState().answer(e))

/** One slice of the store; the same snapshot on the server render, so a static render sees the live state (tests). */
export function useDouble<T>(selector: (s: DoubleStore) => T): T {
  const read = () => selector(doubleStore.getState())
  return useSyncExternalStore(doubleStore.subscribe, read, read)
}

/** The first id the double knows about: cards carry several candidate keys (approval id, ask id, packet key). */
export function usePrediction(...ids: (string | null | undefined)[]): DoublePredicted | undefined {
  return useDouble((s) => {
    for (const id of ids) if (id && s.predicted[id]) return s.predicted[id]
    return undefined
  })
}

export function useAnswered(...ids: (string | null | undefined)[]): DoubleAnswered | undefined {
  return useDouble((s) => {
    for (const id of ids) if (id && s.answered[id]) return s.answered[id]
    return undefined
  })
}
