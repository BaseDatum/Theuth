// Built-in tool switches (206), the twin of `mcp-hide.ts` for the harness's own tools, shared by Settings › Tools ›
// Builtins (`library/Builtins.tsx`), the chat's Tools sheet and the composer's Builtins popover. Two scopes:
// - this chat ("Off here"): the per-session hidden set written by `set_mcp_hidden`, through `useMcpHide` (so a draft
//   chat keeps them in the draft); a whole category is its `cat:<id>` key, which also covers tools registered later;
// - harness-wide ("Off always"): `tools.builtins.disabled` in config.yaml, written by `set_builtins_disabled`.
// A tool's `hidden`/`disabled` flags say only that it is off in that scope, not whether by its name or its category key,
// so "every tool off" stands for "the category key may be set" (as `serverState` does for `mcp:` keys).

import { useQueryClient } from '@tanstack/react-query'
import { type HideOp, useMcpHide } from '@/lib/mcp-hide'
import { useApi } from '@/lib/rpc'
import type { BuiltinCategory, BuiltinTool } from '@/rpc/bindings/method.agent.output'

export type { BuiltinCategory, BuiltinTool }

/** The entry naming a whole category, in either scope. */
export const categoryKey = (id: string): string => `cat:${id}`

/** One tool's or category's switch: on, off for this chat, off everywhere. */
export type BuiltinSwitch = 'on' | 'session' | 'always'

export const toolSwitch = (t: BuiltinTool): BuiltinSwitch => (t.disabled ? 'always' : t.hidden ? 'session' : 'on')

/** A category's 3-way switch: every tool off always; every tool off here and none off always; none off; else mixed
 * (part off always and part off here reads neither "Off here" nor "Off always"). */
export function categorySwitch(c: BuiltinCategory): BuiltinSwitch | 'mixed' {
  const states = c.tools.map(toolSwitch)
  if (states.every((s) => s === 'on')) return 'on'
  if (states.every((s) => s === 'always')) return 'always'
  if (states.every((s) => s === 'session')) return 'session'
  return 'mixed'
}

/** A category's checkbox for this chat (twin of `serverState`), over the tools not off always: none hidden, all
 * hidden, or some of each. A category whose every tool is off always is `off`; its row is disabled. */
export function categoryState(c: BuiltinCategory): 'on' | 'off' | 'mixed' {
  const live = c.tools.filter((t) => !t.disabled)
  if (live.length === 0) return 'off'
  const hidden = live.filter((t) => t.hidden).length
  return hidden === 0 ? 'on' : hidden === live.length ? 'off' : 'mixed'
}

/** This chat only (twin of `serverOps`): off writes the category key, on removes it and every per-tool hide. */
export function sessionCategoryOps(c: BuiltinCategory, hidden: boolean): HideOp[] {
  const key = categoryKey(c.id)
  return hidden ? [{ names: [key], hidden: true }] : [{ names: [key, ...c.tools.map((t) => t.name)], hidden: false }]
}

/** Session ops go to `useMcpHide(session)`; always ops to `set_builtins_disabled(op.names, op.hidden)`. */
export type BuiltinOps = { session: HideOp[]; always: HideOp[] }

const NONE: BuiltinOps = { session: [], always: [] }

/** One scope's set as the tools' flags show it. */
type Scope = (t: BuiltinTool) => boolean
const inSession: Scope = (t) => t.hidden
const inAlways: Scope = (t) => t.disabled

/** Turn a whole category off in one scope: its key, and its per-tool entries go (the key covers them). */
function categoryOff(c: BuiltinCategory, on: Scope): HideOp[] {
  const tools = c.tools.filter(on).map((t) => t.name)
  return [{ names: [categoryKey(c.id)], hidden: true }, ...(tools.length ? [{ names: tools, hidden: false }] : [])]
}

/** Turn a whole category on in one scope: its key and every per-tool entry go; nothing when nothing is off there. */
function categoryOn(c: BuiltinCategory, on: Scope): HideOp[] {
  return c.tools.some(on) ? [{ names: [categoryKey(c.id), ...c.tools.map((t) => t.name)], hidden: false }] : []
}

/** Switch one tool on in one scope. While every tool is off there the category key may be set: trade it for per-tool
 * entries of the others (written first, so nothing shows that should not), or the key would keep this one off. */
function toolOn(c: BuiltinCategory, tool: string, on: Scope): HideOp[] {
  const t = c.tools.find((x) => x.name === tool)
  if (!t || !on(t)) return []
  if (!c.tools.every(on)) return [{ names: [tool], hidden: false }]
  const others = c.tools.filter((x) => x.name !== tool).map((x) => x.name)
  return [...(others.length ? [{ names: others, hidden: true }] : []), { names: [categoryKey(c.id), tool], hidden: false }]
}

/** Move a whole category to `to`. Without a session (Settings with no `?session=`) no session op is produced, and
 * 'session' is not reachable. */
export function categoryOps(c: BuiltinCategory, to: BuiltinSwitch, hasSession: boolean): BuiltinOps {
  if (to === 'session' && !hasSession) return NONE
  if (to === 'always') return { always: categoryOff(c, inAlways), session: hasSession ? categoryOn(c, inSession) : [] }
  return {
    always: categoryOn(c, inAlways),
    session: !hasSession ? [] : to === 'session' ? categoryOff(c, inSession) : categoryOn(c, inSession),
  }
}

/** Move one tool of `c` to `to`, with the same trade as `mcp-hide.ts#toolOps` when its category key may be set. */
export function toolOps(c: BuiltinCategory, tool: string, to: BuiltinSwitch, hasSession: boolean): BuiltinOps {
  if (to === 'session' && !hasSession) return NONE
  const t = c.tools.find((x) => x.name === tool)
  if (!t) return NONE
  if (to === 'always') {
    return { always: t.disabled ? [] : [{ names: [tool], hidden: true }], session: hasSession ? toolOn(c, tool, inSession) : [] }
  }
  return {
    always: toolOn(c, tool, inAlways),
    session: !hasSession ? [] : to === 'on' ? toolOn(c, tool, inSession) : t.hidden ? [] : [{ names: [tool], hidden: true }],
  }
}

/** A draft chat's view: `list_builtins(null)` knows nothing of the draft's hides, so lay them over it. */
export function withDraftHides(cats: BuiltinCategory[], hidden: Record<string, boolean> | undefined): BuiltinCategory[] {
  if (!hidden) return cats
  return cats.map((c) => {
    const off = hidden[categoryKey(c.id)] === true
    return { ...c, tools: c.tools.map((t) => ({ ...t, hidden: off || (hidden[t.name] ?? t.hidden) })) }
  })
}

/** One op and the scope it writes. */
export type ScopedOp = { scope: 'session' | 'always'; op: HideOp }

/** The order `useBuiltinsApply` sends ops in: every turning-off op of both scopes before any turning-on op, each group
 * in its given order, so no tool is on between two calls that is off before and after (and a failed later call
 * leaves things more off, never more on). */
export function applyOrder(ops: BuiltinOps): ScopedOp[] {
  const all: ScopedOp[] = [...ops.always.map((op) => ({ scope: 'always' as const, op })), ...ops.session.map((op) => ({ scope: 'session' as const, op }))]
  return [...all.filter((o) => o.op.hidden), ...all.filter((o) => !o.op.hidden)]
}

/** Apply ops in `applyOrder` (session ops to `session`: `''` the draft, `null` no chat, which has none), then await
 * the refetch of every `list_builtins` view, success or not: a multi-call op that fails halfway still shows the
 * engine's state, and the controls only re-enable on fresh flags. "Off always" changes every chat's list. A real chat
 * id goes with the harness-wide call, so it lands on the engine that owns the chat (the one the list came from); no
 * chat and the draft send two arguments: the local engine. */
export function useBuiltinsApply(session: string | null): (ops: BuiltinOps) => Promise<null> {
  const api = useApi()
  const qc = useQueryClient()
  const hide = useMcpHide(session)
  return async (ops) => {
    try {
      for (const { scope, op } of applyOrder(ops)) {
        if (scope === 'always') await (session ? api.set_builtins_disabled(op.names, op.hidden, session) : api.set_builtins_disabled(op.names, op.hidden))
        else await hide([op])
      }
    } finally {
      await qc.invalidateQueries({ queryKey: ['builtins'] })
    }
    return null
  }
}
