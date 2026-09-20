// The desktop's private-session owner (51 §4.2–4.3, §8.3): one memory-only record of
// the private tree this page owns, and one router-level coordinator that releases it.
// Nothing here — ids, lease secret, view id — may reach localStorage, IndexedDB,
// sessionStorage or the Cache API; a reload forgets the tree and the harness's 30 s
// reconnect deadline closes it.

import type { AnyRouter } from '@tanstack/react-router'
import { useEffect } from 'react'
import { toast } from 'sonner'
import { queryKeys } from '@/lib/rpc'
import { useHarnesses } from '@/link/harnesses'
import type { AgentEvent } from '@/rpc/bindings/AgentEvent'
import type { SessionOpen } from '@/rpc/bindings/SessionOpen'
import type { ConnState } from '@/rpc/client'
import { type Stores, scope } from '@/store/harness-scope'
import type { SessionSlice } from '@/store/sessions'
import { create } from 'zustand'

/** The owning view id (51 §4.1): minted once per page load, memory only. */
export const VIEW = crypto.randomUUID()
/** The harness's fixed reconnect deadline after the owning peer closes (51 §4.3). */
export const GRACE_MS = 30_000

export type Private = {
  /** Harness the tree lives on (`scope` id). */
  harness: string
  root: string
  session: string
  /** Reconnect lease; rotated by every `resume_private`. */
  secret: string
  /** Epoch ms the transport was lost; `undefined` while connected. */
  disconnectedAt?: number
}

type PrivateStore = {
  current: Private | null
  /** The retention warning was acknowledged this page load (51 §1: shown before first use). */
  acknowledged: boolean
  /** In-flight `resume_private` after a reconnect; tree reads wait for it so they land on the rebound owner. */
  resuming: Promise<void> | null
  open(o: SessionOpen): void
  clear(): void
  acknowledge(): void
}

export const usePrivate = create<PrivateStore>()((set) => ({
  current: null,
  acknowledged: false,
  resuming: null,
  open: (o) =>
    set({
      current: {
        harness: useHarnesses.getState().active,
        root: o.root,
        session: o.session,
        secret: o.lease_secret ?? '',
      },
      resuming: null,
    }),
  clear: () => set({ current: null, resuming: null }),
  acknowledge: () => set({ acknowledged: true }),
}))

/** `id` of `/c/<id>` (any search/hash stripped by the caller); `null` for every other path. */
export const chatOf = (pathname: string): string | null => {
  const m = /^\/c\/([^/]+)$/.exec(pathname)
  return m ? decodeURIComponent(m[1]!) : null
}

/** `session` is the root or reaches it through `parent` links / the root's `children` rows. */
export function inTree(root: string, session: string, sessions: Record<string, SessionSlice>): boolean {
  if (session === root) return true
  if (sessions[root]?.children[session]) return true
  for (let id: string | null | undefined = sessions[session]?.parent, hops = 0; id && hops < 32; hops++) {
    if (id === root) return true
    id = sessions[id]?.parent
  }
  return false
}

/**
 * Does moving from `prev` to `next` release the private tree (51 §4.3)? Root→child→parent moves inside `members`
 * do not; dialogs, sheets and search params never change the pathname. Another root, `/`, or a global explorer does.
 */
export const shouldLeave = (prev: string, next: string, members: ReadonlySet<string>): boolean => {
  if (prev === next) return false
  const chat = chatOf(next)
  return chat === null || !members.has(chat)
}

export type Transition = 'none' | 'lost' | 'resume'

/** Transport-state edge for an owned tree: the first loss starts the countdown; the next `open` presents the secret. */
export const transition = (disconnectedAt: number | undefined, state: ConnState): Transition =>
  state === 'open' ? (disconnectedAt === undefined ? 'none' : 'resume') : disconnectedAt === undefined ? 'lost' : 'none'

/** Whole seconds left of the reconnect window; `0` once it has elapsed (the harness may already have closed the tree). */
export const remaining = (disconnectedAt: number, now: number): number =>
  Math.max(0, Math.ceil((disconnectedAt + GRACE_MS - now) / 1000))

/** Disconnect banner text (51 §4.3: the client reports only the bounded window, never deletion). */
export const disconnectText = (secs: number): string =>
  secs > 0
    ? `Disconnected — reconnect within ${secs} s or the private session closes`
    : 'Disconnected — the 30 s reconnect window has elapsed; the harness may already have closed this private session'

const current = () => usePrivate.getState().current

/** The tree's session ids as this client knows them: root + `list_agents` / `SubagentSpawned` rows. */
export function members(p: Private): Set<string> {
  const ids = new Set([p.root])
  for (const id of Object.keys(scope(p.harness).sessions.getState().sessions[p.root]?.children ?? {})) ids.add(id)
  return ids
}

/** Is `session` in the tree this page owns? */
export const isPrivate = (session: string): boolean => {
  const p = current()
  return p !== null && inTree(p.root, session, scope(p.harness).sessions.getState().sessions)
}

/** Forget the tree locally (51 §4.2): every slice, query and the owner record; children before root. */
function discard(st: Stores, p: Private): void {
  const ids = [...members(p)].filter((id) => id !== p.root).concat(p.root)
  usePrivate.getState().clear()
  st.sessions.getState().applyEvents(ids.map((session) => ({ type: 'SessionDeleted', session })))
  for (const id of ids) {
    for (const queryKey of [
      ['session-cost', id],
      ['terminals', id],
      ['pty', id],
      queryKeys.agents(id),
      queryKeys.jobs(id),
      queryKeys.session(id),
      queryKeys.communication(id),
    ])
      st.qc.removeQueries({ queryKey })
  }
  st.qc.removeQueries({ queryKey: queryKeys.artifacts })
  st.qc.removeQueries({ queryKey: queryKeys.searchAll })
}

let router: AnyRouter | null = null
/** Root whose `SessionClosing` was seen; its `SessionDeleted` reports cleanup complete. */
let closing: string | null = null

/** Local state clears at once; the harness reply names the cleanup status, and its absence names the fallback. */
async function leave(p: Private): Promise<void> {
  const st = scope(p.harness)
  discard(st, p)
  closing = p.root
  const api = st.connection.getState().api
  try {
    if (!api) throw new Error('offline')
    const r = await api.leave_session(p.root, p.secret)
    toast(`Private session closed (cleanup ${r.cleanup})`)
  } catch {
    toast.warning(
      'Could not reach the harness to close the private session; it closes on its own once the 30 s reconnect window expires',
    )
  }
}

/** Leave the chat if it is (was) in the tree. */
function exitChat(p: Private): void {
  const chat = chatOf(router?.state.location.pathname ?? '')
  if (chat && inTree(p.root, chat, scope(p.harness).sessions.getState().sessions)) router?.history.push('/')
}

function onTransport(state: ConnState): void {
  const p = current()
  if (!p) return
  switch (transition(p.disconnectedAt, state)) {
    case 'lost':
      usePrivate.setState({ current: { ...p, disconnectedAt: Date.now() } })
      return
    case 'resume': {
      const api = scope(p.harness).connection.getState().api
      if (!api) return
      const job = api.resume_private(p.root, VIEW, p.secret).then(
        (o) => {
          if (current()?.root !== p.root) return
          usePrivate.setState({
            current: { ...p, secret: o.lease_secret ?? p.secret, disconnectedAt: undefined },
            resuming: null,
          })
        },
        () => {
          if (current()?.root !== p.root) return
          discard(scope(p.harness), p)
          toast.error('Private session expired')
          exitChat(p)
        },
      )
      usePrivate.setState({ resuming: job })
      return
    }
    case 'none':
      return
  }
}

/** Resolves once a pending post-reconnect `resume_private` settled; immediate otherwise. Tree reads await this. */
export const resumed = (): Promise<void> => usePrivate.getState().resuming ?? Promise.resolve()

/**
 * The router-level owner coordinator (51 §8.3): mounted once, so a late route unmount can never close a newly
 * opened tree. Releases on navigation out of the tree, harness switch, `SessionClosing`, failed resume and
 * (best effort) page hide; tracks the transport of the owning harness for the 30 s window.
 */
export function usePrivateOwner(r: AnyRouter): void {
  useEffect(() => {
    router = r
    let offConn: (() => void) | undefined
    const watch = (p: Private | null) => {
      offConn?.()
      offConn = p
        ? scope(p.harness).connection.subscribe((c, prev) => {
            if (c.state !== prev.state) onTransport(c.state)
          })
        : undefined
    }
    watch(current())
    const offs = [
      r.subscribe('onResolved', ({ fromLocation, toLocation }) => {
        const p = current()
        if (p && shouldLeave(fromLocation?.pathname ?? toLocation.pathname, toLocation.pathname, members(p)))
          void leave(p)
      }),
      useHarnesses.subscribe((s, prev) => {
        const p = current()
        if (p && s.active !== prev.active && p.harness !== s.active) void leave(p)
      }),
      usePrivate.subscribe((s, prev) => {
        if (s.current?.root !== prev.current?.root) watch(s.current)
      }),
    ]
    const onClosing = (ev: Event) => {
      const e = (ev as CustomEvent<AgentEvent>).detail
      const p = current()
      if (e.type === 'SessionClosing' && p && e.session === p.root) {
        closing = p.root
        discard(scope(p.harness), p)
        toast('Private session closed (cleanup pending)')
        exitChat(p)
      } else if (e.type === 'SessionDeleted' && e.session === closing) {
        closing = null
        toast('Private session closed (cleanup complete)')
      }
    }
    // The socket is going with the page: the call may not arrive, and then the harness's deadline closes the tree.
    const onHide = () => {
      const p = current()
      if (p) void scope(p.harness).connection.getState().api?.leave_session(p.root, p.secret).catch(() => undefined)
    }
    window.addEventListener('theuth:session-closing', onClosing)
    window.addEventListener('pagehide', onHide)
    return () => {
      for (const off of offs) off()
      offConn?.()
      window.removeEventListener('theuth:session-closing', onClosing)
      window.removeEventListener('pagehide', onHide)
      router = null
    }
  }, [r])
}
