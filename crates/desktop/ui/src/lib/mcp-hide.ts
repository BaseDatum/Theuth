// Per-chat MCP hides (`set_mcp_hidden`), shared by the chat's Tools… sheet (`library/Tools.tsx`) and the composer's
// MCP popover (`chat/McpMenu.tsx`). A whole server is hidden by its `mcp:<server>` entry, which the engine applies to
// every tool of that server — tools it registers after a reconnect or restart too — so "off for this chat" sticks.
// A not-yet-created chat (`session === ''`) keeps its hides in the draft (`useDrafts.setHidden`); they are applied
// when the chat is created (`openDraftSession`).

import { useQueryClient } from '@tanstack/react-query'
import { useDrafts } from '@/lib/drafts'
import { useApi } from '@/lib/rpc'
import type { McpServer } from '@/rpc/bindings/method.mcp_2dserver.output'

/** The `set_mcp_hidden` entry naming a whole MCP server. */
export const serverKey = (server: string): string => `mcp:${server}`

/** One `set_mcp_hidden(session, names, hidden)` call. */
export type HideOp = { names: string[]; hidden: boolean }

/** A server's registered tool names (the names `list_mcp` reports, `<server>__<tool>` after a clash). */
export const toolNames = (s: McpServer): string[] => s.tools.flatMap((t) => (t.registered === null ? [] : [t.registered]))

/** A server's checkbox for this chat: every registered tool shown, every one hidden, or some of each. A server
 * with no registered tool (not connected) is `on`; its row is disabled. */
export function serverState(s: McpServer): 'on' | 'off' | 'mixed' {
  const live = s.tools.filter((t) => t.registered !== null)
  const hidden = live.filter((t) => t.hidden).length
  return hidden === 0 ? 'on' : hidden === live.length ? 'off' : 'mixed'
}

/** Turn a whole server off (its `mcp:` entry: later tools stay hidden) or on (the entry and every per-tool hide go). */
export function serverOps(s: McpServer, hidden: boolean): HideOp[] {
  return hidden ? [{ names: [serverKey(s.name)], hidden: true }] : [{ names: [serverKey(s.name), ...toolNames(s)], hidden: false }]
}

/** Switch one tool. Turning one on while the whole server is off trades the server entry for per-tool hides of the
 * others, or the server entry would keep it hidden. */
export function toolOps(s: McpServer, tool: string, hidden: boolean): HideOp[] {
  if (hidden || serverState(s) !== 'off') return [{ names: [tool], hidden }]
  const others = toolNames(s).filter((n) => n !== tool)
  return [{ names: [serverKey(s.name), tool], hidden: false }, ...(others.length ? [{ names: others, hidden: true }] : [])]
}

/** A draft chat's view of a server: `list_mcp(null)` knows nothing of the draft's hides, so lay them over it. */
export function withDraftHides(s: McpServer, hidden: Record<string, boolean> | undefined): McpServer {
  if (!hidden) return s
  const off = hidden[serverKey(s.name)] === true
  return { ...s, tools: s.tools.map((t) => ({ ...t, hidden: t.registered ? off || (hidden[t.registered] ?? t.hidden) : t.hidden })) }
}

/** Apply hide ops to `session` (`''`: the draft; `null`: no chat — refused), in order. */
export function useMcpHide(session: string | null): (ops: HideOp[]) => Promise<null> {
  const api = useApi()
  const qc = useQueryClient()
  return async (ops) => {
    try {
      if (session === null) throw new Error('Open a chat first')
      for (const op of ops) {
        if (session === '') useDrafts.getState().setHidden(op.names, op.hidden)
        else await api.set_mcp_hidden(session, op.names, op.hidden)
      }
    } finally {
      await qc.invalidateQueries({ queryKey: ['tool-categories'] })
    }
    return null
  }
}
