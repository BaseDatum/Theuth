// Owner: U1b (171F B21, B13). What a cell shows that the engine's record does not hold, by `refKey`, so a transcript
// re-render or remount keeps it: the user's unsent edit (F4), the run picked from history, and when Stop was pressed
// (Kill arms 5 s later). Not persisted.
import { createStore, useStore } from 'zustand'
import type { CellRun } from '@/rpc/bindings/CellRun'

type Memory = { edits: Record<string, string>; picked: Record<string, CellRun>; stopped: Record<string, number> }
export const memory = createStore<Memory>(() => ({ edits: {}, picked: {}, stopped: {} }))

const put = <K extends keyof Memory>(field: K, key: string, value: Memory[K][string] | null) =>
  memory.setState((s) => {
    const { [key]: _, ...rest } = s[field]
    return { [field]: value === null ? rest : { ...rest, [key]: value } }
  })

/** The edited code, `null` while the cell shows what the agent wrote. */
export const useEdit = (key: string): string | null => useStore(memory, (s) => s.edits[key] ?? null)
/** Typing back to the original text drops the edit (the cell is unedited again). */
export const setEdit = (key: string, code: string, original: string) => put('edits', key, code === original ? null : code)

export const usePicked = (key: string): CellRun | null => useStore(memory, (s) => s.picked[key] ?? null)
export const setPicked = (key: string, run: CellRun | null) => put('picked', key, run)

/** When Stop was pressed on run `id`, `null` before. */
export const useStoppedAt = (id: string | undefined): number | null => useStore(memory, (s) => (id ? (s.stopped[id] ?? null) : null))
export const setStoppedAt = (id: string, at: number) => put('stopped', id, at)
