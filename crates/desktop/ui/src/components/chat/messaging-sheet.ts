// 188 (D1): the chat's Messaging… sheet: which chat has it open, and its pure parts (the effective line, the root
// scope, the `/c/$session?messaging=1&peer=` params). The sheet itself is `MessagingSheet.tsx`.

import { create } from 'zustand'
import type { CommunicationState } from '@/rpc/bindings/CommunicationState'
import type { SessionSlice } from '@/store/sessions'

type State = {
  /** The chat whose sheet is open; keyed by chat, so a split pane opens only its own. */
  session: string | null
  /** The pending peer request to focus (a message id). */
  peer: string | null
  show(session: string, peer?: string | null): void
  hide(): void
}

export const useMessagingSheet = create<State>((set) => ({
  session: null,
  peer: null,
  show: (session, peer = null) => set({ session, peer }),
  hide: () => set({ session: null, peer: null }),
}))

/** The Effective row: the engine's blocker in words, the ancestor to open when one opted out, and whether the
 * node-wide switch (Settings › Network › Messaging) is what blocks it. */
export type EffectiveLine = { reason: string; ancestor: string | null; nodeOff: boolean }

export function effectiveLine(s: Pick<CommunicationState, 'effective' | 'global' | 'session' | 'opted_out' | 'inherited_from'>): EffectiveLine {
  const ancestor = s.inherited_from && s.inherited_from !== s.session ? s.inherited_from : null
  if (s.effective) return { reason: 'A route still needs the peer in scope, and network peers an explicit grant.', ancestor: null, nodeOff: false }
  if (!s.global) return { reason: 'Blocked: the node-wide switch is off.', ancestor: null, nodeOff: true }
  if (ancestor) return { reason: `Blocked: an ancestor session opted out (${ancestor}).`, ancestor, nodeOff: false }
  return { reason: s.opted_out ? 'Blocked: this chat opted out.' : 'Blocked.', ancestor: null, nodeOff: false }
}

/** Root scope: editable on a loaded root (a top-level chat is its own root); else inherited, with the parent to open. */
export type ScopeView = { kind: 'root' } | { kind: 'inherited'; parent: string | null; note: string }

export function scopeView(slice: Pick<SessionSlice, 'loaded' | 'parent'> | undefined): ScopeView {
  if (slice?.loaded && slice.parent === null) return { kind: 'root' }
  return {
    kind: 'inherited',
    parent: slice?.parent ?? null,
    note: slice?.loaded
      ? 'Inherited from the root of this tree; change it from the root session.'
      : 'Open the chat once to know whether it is a root before editing its scope.',
  }
}

/** `/c/$session` search: `messaging=1` opens the sheet; `peer` (a message id) counts only alongside it. */
export type MessagingSearch = { messaging?: '1'; peer?: string }

export function messagingSearch(s: Record<string, unknown>): MessagingSearch {
  if (s.messaging !== '1' && s.messaging !== 1) return {}
  return { messaging: '1', ...(typeof s.peer === 'string' && s.peer ? { peer: s.peer } : {}) }
}

/** The search with the sheet's params gone: once opened, a reload must not reopen it. */
export function withoutMessaging<T extends MessagingSearch>({ messaging: _messaging, peer: _peer, ...rest }: T): Omit<T, keyof MessagingSearch> {
  return rest
}
