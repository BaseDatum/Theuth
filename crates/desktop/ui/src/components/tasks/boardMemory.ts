// 118: what the Tasks board remembers between visits. Filters per device (localStorage; the URL stays the source of
// truth while the board is open), the horizontal position and the last opened card per window (module memory, gone
// on reload). Pure functions, no React, so the rules are unit-tested in node.

import type { TaskState } from '@/rpc/bindings/TaskState'
import { STATES } from './helpers'

/** Board filters live in the URL so a board survives reload and can be linked. `project: ''` = all projects. */
export type BoardSearch = { project?: string; agent?: string; engine?: string; state?: TaskState; cancelled?: boolean }

/** The one set of rules behind the route's `validateSearch` and `recallBoard`: known keys, no empty agent/node, a real state. */
export function parseBoardSearch(s: Record<string, unknown>): BoardSearch {
  return {
    ...(typeof s.project === 'string' && { project: s.project }),
    ...(typeof s.agent === 'string' && s.agent !== '' && { agent: s.agent }),
    ...(typeof s.engine === 'string' && s.engine !== '' && { engine: s.engine }),
    ...(typeof s.state === 'string' && (STATES as readonly string[]).includes(s.state) && { state: s.state as TaskState }),
    ...(s.cancelled === true && { cancelled: true }),
  }
}

export const BOARD_KEY = 'theuth_tasks_board'

/** The slice of Web Storage the board uses; tests pass a Map-backed one. */
export type BoardStore = { getItem(key: string): string | null; setItem(key: string, value: string): void }

const device = (): BoardStore | null => {
  try {
    return typeof localStorage === 'undefined' ? null : localStorage
  } catch {
    return null
  }
}

/** Keep the board's RAW URL search: `{}` is a legitimate "no filters" and a never-picked project stays "first project". */
export function rememberBoard(search: BoardSearch, storage = device()): void {
  try {
    storage?.setItem(BOARD_KEY, JSON.stringify(search))
  } catch {
    // Private mode or a full quota: the board simply forgets.
  }
}

/** The remembered filters, re-validated by `parseBoardSearch`; `{}` for anything missing or unreadable. */
export function recallBoard(storage = device()): BoardSearch {
  try {
    const raw = storage?.getItem(BOARD_KEY)
    if (!raw) return {}
    const parsed: unknown = JSON.parse(raw)
    return parsed !== null && typeof parsed === 'object' && !Array.isArray(parsed) ? parseBoardSearch(parsed as Record<string, unknown>) : {}
  } catch {
    return {}
  }
}

/** The router's `beforeLoad` cause: a new match, the same match with other search/params, or a hover preload. */
export type EntryCause = 'preload' | 'enter' | 'stay'

/**
 * Where a bare entry to `/tasks` should land: the remembered filters when the board is being ENTERED with no search
 * of its own and there is something to restore; otherwise nowhere. Explicit entries (AgentDetail's `agent`, the Hud's
 * `state`) keep their search, in-place filter edits (`stay`) never snap back — clearing the last filter is `{}` with
 * `stay` — and preloads do not redirect.
 */
export function boardEntryRedirect(cause: EntryCause, search: BoardSearch, remembered: BoardSearch): BoardSearch | null {
  if (cause !== 'enter' || Object.keys(search).length > 0) return null
  return Object.keys(remembered).length > 0 ? remembered : null
}

// Per window, never persisted: a reload or a new window starts at the left like any fresh visit.
const boardScroll: { left: number } = { left: 0 }
let lastOpenedTask: string | null = null

export const rememberBoardScroll = (left: number): void => {
  boardScroll.left = left
}
export const recallBoardScroll = (): number => boardScroll.left
export const rememberOpenedTask = (id: string): void => {
  lastOpenedTask = id
}
/** The task page the user came from, once: the board reads it on its first render and the memory is cleared. */
export function takeOpenedTask(): string | null {
  const id = lastOpenedTask
  lastOpenedTask = null
  return id
}
