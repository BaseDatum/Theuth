// 219 §6.1: the tools that always raise an approval card, even with approvals off. Two scopes: harness-wide (`global`,
// `tools.always_ask`) and this chat (`session`, plus `inherited` from its ancestors, read-only here). The not-yet-created
// chat keeps its own entries in the draft (`DraftSettings.alwaysAsk`) until `openDraftSession` applies them.

import { useQuery, useQueryClient } from '@tanstack/react-query'
import { useDrafts } from '@/lib/drafts'
import { queryKeys, useApi } from '@/lib/rpc'
import { useQuerySupport } from '@/rpc/capabilities'
import type { Api } from '@/rpc/bindings/client'
import type { AlwaysAskState } from '@/rpc/bindings/method.agent.output'

/** A tool row's control: not marked, marked for this chat, marked harness-wide. */
export type AlwaysAskLevel = 'off' | 'session' | 'always'

export const NO_ALWAYS_ASK: AlwaysAskState = { global: [], session: [], inherited: [] }

/** Where `name` stands. `inherited`: marked by an ancestor chat only, so this chat cannot unmark it. */
export function alwaysAskOf(state: AlwaysAskState | undefined, name: string): { level: AlwaysAskLevel; inherited: boolean } {
  if (!state) return { level: 'off', inherited: false }
  if (state.global.includes(name)) return { level: 'always', inherited: false }
  if (state.session.includes(name)) return { level: 'session', inherited: false }
  if (state.inherited.includes(name)) return { level: 'session', inherited: true }
  return { level: 'off', inherited: false }
}

/** How many distinct entries apply to the chat (a name in two scopes counts once). */
export function alwaysAskCount(state: AlwaysAskState | undefined): number {
  return state ? new Set([...state.global, ...state.session, ...state.inherited]).size : 0
}

/** `MCP server` rows mark the whole server with this key (the registry's `mcp_server_key`). */
export const serverKey = (server: string) => `mcp:${server}`

/**
 * `get_always_ask(session)`: `session` a chat id, `''` the draft (the harness-wide list plus the draft's own entries),
 * null for Settings (harness-wide only). Undefined until it answers or on an engine without the method.
 */
export function useAlwaysAsk(session: string | null, enabled = true): AlwaysAskState | undefined {
  const can = useQuerySupport(['get_always_ask'])
  const api = useApi()
  const draft = useDrafts((s) => s.settings.alwaysAsk)
  const q = useQuery({
    enabled: can && enabled,
    queryKey: queryKeys.alwaysAsk(session || null),
    queryFn: () => api.get_always_ask(session || null),
    retry: false,
  })
  if (!q.data) return undefined
  return session === '' ? { ...q.data, session: [...(draft ?? [])].sort() } : q.data
}

/** Mark or unmark `tool` for `session` (`''`: the draft; null: harness-wide). */
export async function setAlwaysAsk(api: Api, session: string | null, tool: string, on: boolean): Promise<void> {
  if (session === '') return useDrafts.getState().setAlwaysAsk(tool, on)
  await api.set_always_ask(session, tool, on)
}

/** Move `tool` from where it stands (`state`) to `to`. Harness-wide and chat entries are independent: `off` clears both;
 * `session` drops a harness-wide entry; `always` keeps the chat's own one (redundant, and removing it would need a card).
 * The new entry goes in before the old one comes out, so a failed (or denied) removal never leaves the tool unmarked. */
export async function setAlwaysAskLevel(api: Api, session: string | null, state: AlwaysAskState | undefined, tool: string, to: AlwaysAskLevel): Promise<void> {
  const global = state?.global.includes(tool) ?? false
  const own = state?.session.includes(tool) ?? false
  if (to === 'always') {
    if (!global) await setAlwaysAsk(api, null, tool, true)
    return
  }
  if (to === 'session' && session !== null && !own) await setAlwaysAsk(api, session, tool, true)
  if (global) await setAlwaysAsk(api, null, tool, false)
  if (to === 'off' && session !== null && own) await setAlwaysAsk(api, session, tool, false)
}

/** Every `get_always_ask` reading refetches (prefix `['always-ask']`): children inherit a chat's entries and every chat
 * sees the harness-wide ones. */
export function useInvalidateAlwaysAsk(): () => Promise<void> {
  const qc = useQueryClient()
  return () => qc.invalidateQueries({ queryKey: ['always-ask'] })
}
