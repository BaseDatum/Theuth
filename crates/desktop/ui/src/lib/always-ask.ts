// 219 §6.1: the tools that always raise an approval card, even with approvals off. Two scopes: harness-wide (`global`,
// `tools.always_ask`) and this chat (`session`, plus `inherited` from its ancestors, read-only here). The not-yet-created
// chat keeps its own entries in the draft (`DraftSettings.alwaysAsk`) until `openDraftSession` applies them.

import { useQuery, useQueryClient } from '@tanstack/react-query'
import { useDrafts } from '@/lib/drafts'
import { queryKeys, useApi } from '@/lib/rpc'
import { useQuerySupport } from '@/rpc/capabilities'
import type { Api } from '@/rpc/bindings/client'
import type { AlwaysAskState } from '@/rpc/bindings/method.agent.output'
import type { AlwaysAskWhy } from '@/rpc/bindings/event.rpc_2estream.output'
import { backendLabel, parseAskKey } from '@/components/library/secrets/labels'

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

/** Where an entry is set: `tools.always_ask` (every chat), this chat's list, or an ancestor chat's. */
export type AlwaysAskScope = 'every chat' | 'this chat' | 'a parent chat'

/** Every entry that applies, once, with where it is set (harness-wide wins over the chat's own). */
export function alwaysAskEntries(state: AlwaysAskState | undefined): { entry: string; scope: AlwaysAskScope }[] {
  if (!state) return []
  const out = new Map<string, AlwaysAskScope>()
  for (const [list, scope] of [[state.global, 'every chat'], [state.session, 'this chat'], [state.inherited, 'a parent chat']] as const) {
    for (const entry of list) if (!out.has(entry)) out.set(entry, scope)
  }
  return [...out].map(([entry, scope]) => ({ entry, scope }))
}

/** Where the entries are set, for a line under their summary: `Set for every chat (Settings › Approvals)`, … */
export function alwaysAskWhere(entries: readonly { scope: AlwaysAskScope }[]): string {
  const where: Record<AlwaysAskScope, string> = {
    'every chat': 'every chat (Settings › Approvals)',
    'this chat': 'this chat (its Tools)',
    'a parent chat': 'a parent chat',
  }
  return `Set for ${[...new Set(entries.map((e) => e.scope))].map((s) => where[s]).join(', ')}`
}

const DIRECTION_ORDER = ['read', 'write', 'send', 'delete']

/**
 * The entries as one line a person can read, a connection's `secret:` keys folded into one item and an `mcp:<server>`
 * key spelled out: `vw (Bitwarden): read, write, send, delete; secret_delete`. Order: first appearance.
 */
export function alwaysAskSummary(entries: readonly string[]): string {
  const dirs = new Map<string, string[]>()
  const items: string[] = []
  for (const e of entries) {
    const k = parseAskKey(e)
    if (!k) {
      items.push(e.startsWith('mcp:') ? `MCP server ${e.slice(4)}` : e)
      continue
    }
    const conn = `${k.conn} (${backendLabel(k.backend)})`
    if (!dirs.has(conn)) {
      dirs.set(conn, [])
      items.push(conn)
    }
    dirs.get(conn)?.push(k.dir)
  }
  return items
    .map((i) => {
      const d = dirs.get(i)
      return d ? `${i}: ${[...d].sort((a, b) => DIRECTION_ORDER.indexOf(a) - DIRECTION_ORDER.indexOf(b)).join(', ')}` : i
    })
    .join('; ')
}

/**
 * Why an always-ask card asks, for the card itself: the matching entries (and that `tool` is not one of them, when only
 * a secret connection or its server is), or the mandatory gate, which no switch turns off.
 */
export function alwaysAskWhyText(why: AlwaysAskWhy, tool: string): string {
  switch (why.kind) {
    case 'entries': {
      const itself = why.entries.includes(tool) ? '' : `, not for ${tool} itself`
      return `Always ask is on for ${alwaysAskSummary(why.entries)}${itself}. Change it in Settings › Approvals or this chat's Tools.`
    }
    case 'config_write':
      return "Always asks: it writes theuth's own config. This gate cannot be turned off."
    case 'mcp_admin':
      return 'Always asks: it changes MCP servers, and always-ask entries exist (re-adding a server must not shed them).'
    case 'unreadable':
      return "Always asks: this chat's always-ask list could not be read, so every change asks until it can."
  }
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
