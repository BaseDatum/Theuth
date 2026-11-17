import { observedLabel, stripRows } from '@/lib/session-tree'
import type { AgentInfo } from '@/store/ui-events'
import type { AgentState } from "@/rpc/bindings/method.agent.output"

/** How long a session that stopped being live stays listed (fading) before it drops out (100 D2). */
export const LINGER_MS = 700

/** How many dots an org-chart card's strip shows before a `+N` tail (100 D4). */
export const CARD_STRIP_CAP = 8

/** The live rows of a desk's subtree (any depth) in spawn order — the strip rule with no cap. */
export const liveRows = (children: Readonly<Record<string, AgentInfo>>): AgentInfo[] => stripRows(children, Number.POSITIVE_INFINITY).shown

/** The sidebar tree's phrasing of a row's state. */
export const stateText = (s: AgentState): string => (s.state === 'awaiting' ? `awaiting ${s.from}` : s.state === 'approval' ? `approval: ${s.tool}` : s.state)

/** A child row's model slot: `<harness> · <model>` for a harness child, the model for a native one, `model unknown` when none is known — never the harness name alone. */
export const modelLabel = (row: Pick<AgentInfo, 'agent' | 'model'>): string => {
  const model = row.model || 'model unknown'
  return row.agent ? `${row.agent} · ${model}` : model
}

/** A strip dot's title (composer strip, org-chart card): `name · state · model`; a watch-only child (141 §9.2c) reads `watching · <harness state>`. */
export const dotLabel = (row: AgentInfo): string =>
  `${row.name} · ${row.observed ? `watching · ${observedLabel[row.observed.state]}` : stateText(row.state)} · ${modelLabel(row)}`

/** What a list shows: `ids` in first-seen order; `leaving` maps the ids no longer live to the clock when they left. */
export type Lingering = { ids: string[]; leaving: Record<string, number> }

export const NO_LINGERING: Lingering = { ids: [], leaving: {} }

/**
 * Linger, then fade: an id that was shown and is no longer in `liveIds` stays for `ms` after it left (`leaving`
 * records when), then drops; one that comes back is un-marked; new live ids append. Returns `prev` itself when
 * nothing changed, so a store beat with the same live set costs no render.
 */
export function linger(prev: Lingering, liveIds: readonly string[], now: number, ms = LINGER_MS): Lingering {
  const isLive = new Set(liveIds)
  const seen = new Set(prev.ids)
  const ids: string[] = []
  const leaving: Record<string, number> = {}
  let changed = false
  for (const id of prev.ids) {
    const left = prev.leaving[id]
    if (isLive.has(id)) {
      if (left !== undefined) changed = true
      ids.push(id)
      continue
    }
    const at = left ?? now
    if (now - at >= ms) {
      changed = true
      continue
    }
    if (left === undefined) changed = true
    ids.push(id)
    leaving[id] = at
  }
  for (const id of liveIds) {
    if (seen.has(id)) continue
    ids.push(id)
    changed = true
  }
  return changed ? { ids, leaving } : prev
}
