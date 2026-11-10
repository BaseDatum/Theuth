// 183 §5.6: a chat this window just created on another machine. Its row reaches this engine only when the session log
// replicates (seconds on a slow mesh); the engine forwards every call for it by id meanwhile. Here: the composer says
// "Starting on <machine>…" and holds the next send until the session's first event (design 183 §13 row 11), and the
// chat route retries a "no such session" read for `YOUNG_MS` before showing the chat as unreachable in place — never
// sending the user home. In memory only: a relaunch has nothing young.

import { create } from 'zustand'

/** How long a chat created elsewhere is retried by the route before it is called unreachable. */
export const YOUNG_MS = 30_000

/** `machine`: its mesh name; `at`: when it was created (or last retried); `heard`: an event of it arrived. */
export type RemoteStart = { machine: string; at: number; heard: boolean; unreachable: boolean }

type Store = {
  by: Record<string, RemoteStart>
  /** `session` was just created on `machine` (or Try again restarts its retries). */
  begin(session: string, machine: string, at?: number): void
  /** An event of `session` arrived: its machine took it, the composer is free and any unreachable card goes. */
  heard(session: string): void
  /** The route's retries ran out: the chat shows the unreachable card. */
  unreachable(session: string): void
  forget(session: string): void
}

export const useRemoteStart = create<Store>()((set, get) => {
  const patch = (session: string, p: Partial<RemoteStart>) => {
    const cur = get().by[session]
    if (cur) set({ by: { ...get().by, [session]: { ...cur, ...p } } })
  }
  return {
    by: {},
    begin: (session, machine, at = Date.now()) => set({ by: { ...get().by, [session]: { machine, at, heard: false, unreachable: false } } }),
    heard: (session) => {
      const cur = get().by[session]
      if (cur && (!cur.heard || cur.unreachable)) patch(session, { heard: true, unreachable: false })
    },
    unreachable: (session) => patch(session, { unreachable: true }),
    forget: (session) => {
      const { [session]: _, ...by } = get().by
      set({ by })
    },
  }
})

/** The chat route's answer to "no such session" for its own `session` after `tries` retries: an id nobody created here
 * goes home (deleted, stale link); a chat just created elsewhere retries with backoff, then becomes unreachable. */
export function onNoSuchSession(session: string, tries: number, now = Date.now()): { retry: number } | 'unreachable' | 'home' {
  const r = useRemoteStart.getState().by[session]
  if (!r) return 'home'
  if (now - r.at < YOUNG_MS) return { retry: Math.min(500 * 2 ** tries, 8_000) }
  return 'unreachable'
}
