// 183 §5.6: a chat this window just created on another machine, until that machine is heard from. Its row reaches this
// engine only when the session log replicates (seconds on a slow mesh); the engine forwards calls for it by id
// meanwhile. Here: the first send waits for the chat's follow stream (`live`, so its first events are not missed), the
// composer says "Starting on <machine>…" and holds the next send until the chat's first event or row, at most
// `YOUNG_MS` (design 183 §13 row 11), and the chat route retries a "no such session" read for `YOUNG_MS` before showing
// the chat as unreachable in place — never sending the user home. The entry goes on the first event. In memory only.

import { create } from 'zustand'
import { isNoSuchSession } from '@/lib/no-such-session'

/** How long a chat created elsewhere is retried by the route (and holds the composer) before it is called unreachable. */
export const YOUNG_MS = 30_000

/** `machine`: its mesh name; `at`: when it was created (or last retried); `live`: its follow stream is up. */
export type RemoteStart = { machine: string; at: number; live: boolean; unreachable: boolean }

type Store = {
  by: Record<string, RemoteStart>
  /** `session` was just created on `machine` (or Try again restarts its retries). */
  begin(session: string, machine: string, at?: number): void
  /** `FollowState{live}` for `session`: its machine's events reach this window from now on. */
  live(session: string): void
  /** The route's retries ran out: the chat shows the unreachable card. */
  unreachable(session: string): void
  /** An event of `session` arrived (its machine took it) or its row is here: nothing is starting any more. */
  forget(session: string): void
}

export const useRemoteStart = create<Store>()((set, get) => {
  const patch = (session: string, p: Partial<RemoteStart>) => {
    const cur = get().by[session]
    if (cur) set({ by: { ...get().by, [session]: { ...cur, ...p } } })
  }
  return {
    by: {},
    begin: (session, machine, at = Date.now()) => set({ by: { ...get().by, [session]: { machine, at, live: false, unreachable: false } } }),
    live: (session) => patch(session, { live: true }),
    unreachable: (session) => patch(session, { unreachable: true }),
    forget: (session) => {
      if (!(session in get().by)) return
      const { [session]: _, ...by } = get().by
      set({ by })
    },
  }
})

/** Resolves once `session`'s follow stream is up (or it was heard from, or never started here), at most `ms` later. */
export function untilLive(session: string, ms: number): Promise<void> {
  const ready = (s: Store) => s.by[session]?.live !== false
  const { promise, resolve } = Promise.withResolvers<void>()
  if (ready(useRemoteStart.getState())) {
    resolve()
    return promise
  }
  const timer = setTimeout(resolve, ms)
  const off = useRemoteStart.subscribe((s) => {
    if (ready(s)) resolve()
  })
  void promise.finally(() => {
    clearTimeout(timer)
    off()
  })
  return promise
}

/**
 * The chat route's handling of a failed read of its own `session` (`useSessionLoad`; `tries` is the route's retry count,
 * `now` for tests). Any other error toasts. "no such session" for an id nobody created here goes `home` silently (a deleted
 * chat, a stale link); for a chat just created on another machine it never does: it `retry`s with backoff for
 * `YOUNG_MS`, then marks it unreachable (the card).
 */
export function reportLoadError(
  e: unknown,
  session: string,
  tries: { current: number },
  io: { home(): void; toast(message: string): void; retry(ms: number): void },
  now = Date.now(),
): void {
  if (!isNoSuchSession(e)) {
    io.toast(e instanceof Error ? e.message : String(e))
    return
  }
  const r = useRemoteStart.getState().by[session]
  if (!r) io.home()
  else if (now - r.at < YOUNG_MS) io.retry(Math.min(500 * 2 ** tries.current++, 8_000))
  else useRemoteStart.getState().unreachable(session)
}
